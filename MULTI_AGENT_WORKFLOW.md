# Multi-agent development workflow

Use this workflow for a development request in this repository. In a Codex chat, select **GPT-6.2 Astra · High** and send the prompt below with your request. From the repository root, you can also launch an interactive Codex session with `codex -m gpt-6.2-astra -c model_reasoning_effort=high`.

```text
Use MULTI_AGENT_WORKFLOW.md for this request. Act as the planner and spawn the named project agents for implementation, assets when needed, and testing. Follow the workflow through verification.

Request: <describe the change>
```

| Role | Model | Reasoning effort |
| --- | --- | --- |
| Planner, the main Codex session | GPT-6.2 Astra | High (`high`) |
| `programmer` | GPT-6.2 Sol | High (`high`) |
| `artist` | GPT-6.2 Sol | Medium (`medium`) |
| `tester` | GPT-6 Luna | Extra High (`xhigh`) |

Use the named project agents configured under `.codex/agents/`. If an assigned agent, model, or required tool is unavailable, record the blocker and continue any independent work. Keep verification pending until the required agent can run it.

## 1. Plan the request

The planner reads the request and inspects the relevant repository code, assets, and tests. Limit the planner's work to analysis, planning, coordination, and recording results. The programmer and artist own implementation. The tester owns execution of the validation plan.

Before assigning work, the planner records:

- The requested outcome, acceptance criteria, and applicable project constraints.
- The algorithms and implementation steps, including relevant entry points, interfaces, dependencies, and edge cases.
- Any assets to generate, with their intended use, output paths, formats, and applicable specifications such as dimensions, channels, color space, or coordinate conventions. Record when the request needs no new assets.
- Each validation check with an ID, the criterion it covers, the exact command or manual steps, required fixtures or environment, and expected results. Include a rendering or asset integration check when the requested change affects those paths.

## 2. Clarify and implement

The planner sends the programmer the implementation steps, interfaces, relevant acceptance criteria, and file ownership. If the plan calls for new assets, it sends the artist the asset brief and identifies which code consumes the outputs. The two agents can work in parallel on independent files once they agree on the integration contract. Sequence any shared-file edits and any work that depends on a finished asset.

- **Programmer.** Review the plan and ask the planner to confirm assumptions that affect the implementation or to resolve open questions. Once the planner responds, implement the agreed code, asset integration, and any test code the plan calls for. Report changed files, material decisions, and remaining blockers.
- **Artist.** Before generating assets, ask the planner to confirm the format, output paths, and any assumptions that affect generation or integration. Once confirmed, generate the deliverables and report their paths, actual specifications, and any integration notes or blockers.

Send questions through the agent messaging tools and wait for answers before starting dependent work. If an agent ends its turn with questions, the planner supplies the answers in a follow-up to that same agent. The planner records any changes to the plan and relays changes to shared interfaces to both agents.

## 3. Run the test plan

After the programmer and any assigned artist report completion and the integrated files are available, the planner assigns the tester the current test plan and both agents’ handoffs. The tester runs every required check, continuing through independent checks after a failure. For each check, it reports the ID, command or steps used, expected and observed results, and a status of **Pass**, **Fail**, or **Blocked**. A failure report includes the relevant output or artifact path and reproduction details. A blocked report identifies the missing prerequisite.

## 4. Fix, retest, and close

1. The planner records the test results in the plan. For each failure, it sends the evidence and expected behavior to the agent responsible for the fix. Send code, test harness, and integration issues to the programmer and asset defects to the artist. For failures spanning both, assign each agent a specific part of the fix and confirm the shared interface. If the plan itself is incorrect, the planner corrects the check against the original acceptance criterion and gives the tester the revised steps.
2. Once the responsible agents report completion, ask the tester to rerun the failed checks against the updated files, along with any previously passing checks affected by the fix. If a check is blocked, resolve the prerequisite where possible and then have the tester run it. Use the latest result for each check.
3. Repeat until every required check has passed or a blocker prevents further progress. When all required checks pass, the planner marks the plan **Complete**, records the final results and any fix-and-retest outcomes, reports the verified outcome, and stops. If a blocker remains, report it and leave the plan incomplete.
