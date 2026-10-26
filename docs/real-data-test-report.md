# Real-data test report

> **Run:** 2026-10-26 · release build of `prswt-cli` · driven over MCP stdio

The engine was run on schedules that other people made and published. The point was to check behaviour on files we did not write. The files stay outside the repository (clean-room rule). Each is named here by its public source so the run can be repeated.

## Files

| File | Source (public GitHub) | Made by | Size |
|---|---|---|---|
| Harbour Pointe Assisted Living Center | `djouallah/Xer-Reader-PowerBI` | P6 17.12 export | 131 activities, 212 links, 29 resources, 417 assignments |
| Project_Schedule | `RMF112018/hb-intel` | XER from a non-P6 tool | 281 activities, 261 links |
| EventsExample2002 | `aspose-tasks-cloud/Aspose.Tasks-Cloud-SDK-for-Python` | Aspose (dates carry seconds) | 83 activities, 86 links |
| Sample Refueling Outage | `idaholab/DACKAR` | Hand-written XER | 4 activities |
| rst_advanced_sample_project_MSP2010 | `oeg-upm/cogito_data_repository` | MS Project 2010 | 33 tasks, 36 links |
| SETuP-plan | `pjmaker/taskjuggler-template` | MS Project XML | 6 tasks |
| AlphaX_Project_Schedule | `erichokh65/MIL-AI` | Generated MSPDI (OutlineLevel only) | 138 tasks, 454 links |

## Results

| Check | Result |
|---|---|
| Harbour: project finish vs P6 `scd_end_date` | **Equal** to the minute (2014-09-24 10:40) |
| Harbour: data date, critical path end | Equal |
| Harbour: budget (BAC) vs sum of P6 `target_cost` | **Equal** (6,097,581.10), after fix 3 below |
| Harbour: actual cost vs sum of P6 actual costs | **Equal** (650,946.02) |
| Harbour: XER export → import | Same finish |
| Harbour: timing | Import 6 ms; schedule 1 ms; leveling 25 ms; 2,000 Monte Carlo iterations 28 ms |
| hbintel: ES, LF, total float of 281 activities | All equal |
| rst (MS Project 2010): start, finish, late finish of 33 tasks; project finish | All equal |
| SETuP: 6 tasks | All equal |

## Defects found and fixed

1. **ALAP chains (CPM-09).** An ALAP activity whose successor was also ALAP did not slide. Activities were visited in index order, and free float was read before the successor moved. They are now visited in reverse topological order, and free float is read from the current link slack. Oracle: `cpm_09_alap_chain_slides_together`.
2. **XER from other exporters (FMT-01).** Three cases made the import fail:
   - no `plan_start_date` (falls back to the data date, then the earliest activity date);
   - dates with seconds;
   - a blank `PROJECT.proj_id`.

   Test: `fmt_01_tolerates_files_from_other_exporters`.
3. **XER resource rates (FMT-01).** P6 keeps prices and limits in `RSRCRATE` (dated, five price types). Only `RSRC.cost_per_qty` was read, so Harbour had no cost. The import now also reads:
   - dated rates and limits from `RSRCRATE`;
   - the assignment price type;
   - actual units and actual cost;
   - remaining units.

   Test: `fmt_01_dated_rates_and_assignment_actuals`.
4. **MSPDI without `OutlineNumber` (FMT-02).** The field is optional; AlphaX has only `OutlineLevel`, and the import failed. Outline numbers are now rebuilt from the task order. Test: `fmt_02_outline_rebuilt_from_levels`.

## Differences that are not defects

- **Harbour, off-critical activities.** Some stored early dates do not fit the stored durations. EC2380 is 144 h, but its stored early dates span 96 h. P6's own remaining dates (`reend_date`) agree with ours. The durations were edited after P6 last scheduled the file.
- **In-progress early start.** P6 writes the remaining start (the data date) as `early_start_date`. We report the actual start; the remaining start is the data date.
- **hbintel zero-duration tasks.** The file has early finish 17:00 before early start 08:00 the next day; we put both at 08:00.
- **Aspose.** The file was written by Aspose's own engine, not P6. Its milestones sit at 17:00 of the previous day, and expected finishes fix the duration and move the start. Neither is confirmed P6 behaviour, so neither was copied (open question: start-milestone time after an FS link).
- **AlphaX.** The tasks start a month before the project start and do not follow their own links: the dates were generated, not scheduled by MS Project.
