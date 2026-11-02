# How the AI evaluates and plans CAM work

This is the design for issue #11348: CAM that an AI assistant drives. It says what the
assistant does at each step of a machining job, which facts and checks SolidCraft gives it,
and how it knows the job is right before anything reaches a machine.

## The idea in one paragraph

SolidCraft measures and checks; the assistant judges and explains; the person decides. The
engine never chooses a machining strategy silently. It reports facts about the part (what
features it has, what can be reached from where, what limits the tools), checks every toolpath
(gouges, collisions, machine limits, material left), and scores a whole job with numbers the
assistant can compare. The assistant uses those facts and its skills to plan the job, builds
it with `cam.*` commands, reads the checks, fixes what they find, and repeats until the job
passes. Then it shows the person the plan, the risks and the run time, and posts only when
they approve.

## The loop

```mermaid
flowchart LR
    U[1 Understand] --> E[2 Evaluate the part]
    E --> P[3 Plan]
    P --> B[4 Build]
    B --> C[5 Check the job]
    C -- issues left --> R[6 Refine]
    R --> P
    C -- every safety check passes --> A[7 Approve and post]
    A --> L[8 Learn]
```

Every step has a command whose result is a notice (`info`, `values`, `issues` with stable
codes and a `suggestion`), so the assistant never has to read prose or guess.

## 1. Understand the job

What the assistant needs before it plans, and where each answer comes from:

| Fact | Source |
|---|---|
| The part: bodies, size, features | the design (`inspect_design`, step 2's analysis) |
| Material | the person (the guide skill asks); default from the tool kit |
| Machine and its options (4th axis, laser) | the person; presets in `cam.machine.list` |
| Stock: size, already square or not | the person; else a relative box around the part |
| Workholding: vise, clamps, tape, chuck | the person |
| What matters most: finish, accuracy of named features, run time | the person |
| How many parts | the person (one part favours fewer tools; a batch favours run time) |

**New: `cam.job.set {material, workholding, priority, quantity, tolerances}`** keeps these
answers in the design, next to the setups. Every later check reads them: feeds come from the
material, workholding limits which faces can be cut and how hard, and tolerances mark the
features that need a finishing pass. A job without them still works, with stated defaults
reported as `info` issues, so the assistant knows what it assumed.

## 2. Evaluate the part

**New: `cam.analyze {bodies?, machine?, stock?}`**, a machinability report. It is facts only,
no decisions:

- **Features**, each with an id, the faces it covers and its numbers:
  - flat floors and their depths from the top;
  - pockets (open or closed), with depth, narrowest width and smallest inside corner radius;
  - holes, with diameter, depth, through or blind, and whether the size matches a tap drill;
  - bosses and islands with their heights;
  - walls, with height and draft;
  - free-form regions, with their curvature range;
  - edges that are chamfered or filleted.
- **Access.** For each direction a setup could face (+Z, −Z, ±X, ±Y), which faces a 3-axis
  tool reaches. What no direction reaches is an `undercut` (a T-slot, a dovetail).
- **Limits** each feature puts on the tools:
  - the smallest inside corner radius sets the largest tool that finishes it;
  - the deepest narrow feature sets the stick-out and flute length;
  - thin walls limit step-down.
- **Fit.** Does the part fit the machine's work area, and does a stock that size fit?

Issues carry codes the plan acts on:
- `undercut`: needs another setup, the 4th axis, or a special tool.
- `sharp_inside_corner`: no round tool can make it, so ask the designer for a radius.
- `deep_narrow`: needs a long tool, or can't be reached at all.
- `thin_wall`: cut lightly and leave it until late.
- `exceeds_work_area`: the part or stock doesn't fit the machine.
- `needs_second_side`: some faces can only be reached from below.

Much of this already exists inside the CAM crate: the height maps and drop-cutter used for 3D
toolpaths, the boundary code that finds faces and holes, and the stock simulation. Analysis
puts those together in one report.

## 3. Plan

The assistant writes the plan; the engine can propose a starting point.

**New: `cam.plan.propose {}`** turns the analysis into a baseline plan from plain rules. Each
step in it has a `why` and the feature ids it covers:
- the fewest setups that reach every feature;
- tools chosen from the limits;
- largest material removal first (face → adaptive rough → rest rough → finish floors and walls
  → surfaces → holes → chamfers and engraving).

It is a starting point the assistant improves with its skills, the person's priorities and its
own judgement. It is not the answer.

A plan is data, not prose:

```json
{"setups": [{"name": "Top", "direction": "+z", "workholding": "vise", "origin": "stock top front-left",
   "operations": [{"type": "face", "tool": "6 mm flat", "covers": ["top"], "why": "a known top for every depth"},
                  {"type": "adaptive2d", "tool": "6 mm flat", "covers": ["pocket-1", "pocket-2"], "why": "bulk removal at a steady load"}]}],
 "uncovered": [{"feature": "slot-3", "why": "undercut: needs a T-slot cutter"}]}
```

**New: `cam.plan.apply {plan}`** builds the plan in one call: tools, setups and operations, each
operation's geometry bound to the faces of the features it covers. The result maps each feature
id to an operation id. The coverage map is what makes step 5's checks per feature.

The assistant shows the plan to the person in plain words, one sentence of why per step, and
waits for a go-ahead before building. The `solidcraft-makera-guide` skill already does this for
Makera machines.

## 4. Build

`cam.plan.apply`, or the existing commands one at a time, then `cam.generate`. Operations are a
parametric chain: changing an operation, its tool, its setup or its machine clears the
toolpaths that depend on it. So after any change, the assistant regenerates only what changed.

## 5. Check the job

**New: `cam.evaluate {setup?}`** scores the job. Its checks fall into four groups.

| Group | Checks | Gate |
|---|---|---|
| Safety | gouges (where and how deep), rapid moves through stock or part, holder and spindle collisions, travel, rpm and feed limits, tool reach, a tool the machine can hold | must be zero to post |
| Completeness | per feature: covered or not, material left above its allowance (volume and where), what is knowingly left (an undercut) | every feature covered or explained |
| Quality | scallop height against the target finish, stock to leave consistent before finishing, tolerance features finished last with a light pass | reported against the job's targets |
| Efficiency | cycle time, time cutting air, tool changes, chip load and engagement within the tool's range | compared between variants |

The result is one notice with a score per group and every issue, each with a `suggestion` that
clears it, so the next step is mechanical when it can be. It is deterministic: the same job
gives the same numbers, so two variants can be compared honestly.

Most of the safety and completeness checks already exist and run during generation and
simulation: gouge cells, rapid collisions, machine limits, tool too short, and stock left
after simulation. What's missing:
- leftover material per feature;
- air-cutting time;
- engagement;
- the one score the assistant can compare.

## 6. Refine

The assistant applies suggestions, changes a tool or a strategy, regenerates the stale steps
and checks again. To keep this bounded and honest:

- **Stop when** every safety check passes, every feature is covered or explained, and a further
  change improves run time or finish by less than a few percent.
- **Try a variant** (another roughing tool, another strategy) and keep the better result.
  `cam.evaluate` returns the job's signature, so the assistant can tell which variant each set
  of numbers belongs to.
- **After a few rounds without progress, stop and ask the person**, saying what is still wrong
  and what the choices are (another setup, another tool, a design change such as a corner
  radius). Never post around an unresolved safety issue.

## 7. Approve and post

Before posting, the assistant gives the person a summary:
- the plan, in plain words;
- the run time;
- the tools to load, in order;
- the workholding;
- where to set zero (the setup's origin, and how a Makera probes it);
- what was knowingly left uncut;
- any warnings.

`cam.post` checks the safety gates again and refuses with the issue codes if any fail. After
approval, it writes the program for the machine's dialect.

## 8. Learn

- **Benchmark parts.** A set of test parts, from simple to hard: pockets, islands, slopes,
  triangles, an undercut, a dome and holes, like the test part used so far. Each has the
  checks a good job must pass. `cam.plan.propose` runs on them in the tests, so a change that
  makes the baseline worse fails the build.
- **Assistant evaluations.** The same parts are given to an assistant using the skills. It is
  scored on safety gates passed, features covered, run time, rounds needed, and questions
  asked. This is how the skills improve: change the skill, and the score says whether it
  helped.
- **Shop feedback.** **New: `cam.job.feedback {operation, result}`** records what happened at
  the machine: chatter, a broken tool, a poor finish, or "fine". It adjusts that machine and
  material's feeds for next time, stored with the tool library, not in the design.

## Who does what

| | Engine (`cam.*` commands) | Assistant (skills) | Person |
|---|---|---|---|
| Understand | stores the job's intent | asks the questions | answers, states priorities |
| Evaluate | analysis: features, access, limits | reads it, spots what matters | — |
| Plan | baseline proposal | improves it, explains it | approves |
| Build | builds, generates, regenerates what changed | issues the commands | — |
| Check | every check, one score | reads the issues | — |
| Refine | suggestions per issue | applies them, compares variants, knows when to stop | decides when asked |
| Post | refuses unsafe programs | summarises how to run it | approves, runs the machine |
| Learn | benchmarks, feeds memory | — | reports how the cut went |

## Building on whatever CAM core lands

This layer needs only a few things from the CAM core:
- operations that produce toolpaths;
- a stock simulation;
- per-operation notices.

If another CAM core reaches `main` first (#2652, #5751, #2881), the analysis, plan, evaluation
and learning commands sit on top of it instead, through a small set of functions. They don't
compete with it. The fork's `cam` branch has a full core today, which is what these commands
are developed and tested against.

## Order of work

1. **`cam.analyze`**: features, access and limits, with tests on the benchmark parts. Every
   later step needs it.
2. **`cam.evaluate`**: one score over the checks that exist, plus per-feature leftover
   material and air-cutting time.
3. **`cam.job.set`**: the job's intent, read by analysis, evaluation and feeds.
4. **`cam.plan.propose` and `cam.plan.apply`**: the baseline plan and the coverage map.
5. **Skills**: `solidcraft-cam` and the Makera guide are rewritten around the loop above.
6. **Benchmarks and assistant evaluations.**
7. **`cam.job.feedback`.**

Each one is a pull request of its own under #11348, with screenshots of what it shows in the
Manufacture workspace.
