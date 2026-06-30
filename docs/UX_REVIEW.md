# Chromium UX Review

This is the repeatable review loop for checking whether the operator UI is
actually usable by one human running the trading workflow. It is not a visual
polish pass. The reviewer must use Chromium against the running app and judge
whether each screen makes the next human decision obvious, auditable, and safe.

Run it after the dev stack is up:

```bash
make dev
make ux-review
```

By default the harness reviews `http://localhost:5173` and the symbols
`OKTA,NVDA,CRDO`. Override when needed:

```bash
make ux-review UX_REVIEW_BASE_URL=http://localhost:5173 UX_REVIEW_SYMBOLS=OKTA,NVDA
```

Artifacts are written under `.runtime/ux-review/<timestamp>/`:

- `report.md` - reviewer report scaffold and route inventory.
- `*.png` - desktop and narrow screenshots.
- `*.json` - captured headings, controls, text snippets, and data-testids.

## Designer Prompt

Use this prompt for a senior UX engineer/product designer agent reviewing the
site. The agent should use Chromium, inspect the generated screenshots/JSON, and
interact with the running app directly.

```text
You are a senior product designer and UX engineer reviewing a single-operator
trading intelligence workstation. Your job is to find places where a real human
operator would be confused, slowed down, overconfident, or unable to audit the
system's proposed action.

Use Chromium against the running app. Do not review static screenshots only.
Treat this as an operational workflow review, not a marketing-site critique.

Safety constraints:
- Do not place broker orders.
- Do not use live-trading credentials.
- Do not perform destructive database operations.
- Shadow automation approval is allowed only in a local/dev environment when
  the reviewer explicitly says it is part of the tested path.

Core user:
- One technically competent operator.
- They want to understand what the system believes, why a symbol is queued,
  what action is being requested, what happens after approval, and where to
  audit entries, exits, proofs, positions, blockers, and outcomes.

Primary workflows to inspect:
1. Open the app and answer: what needs my attention first?
2. Open a symbol from the attention/workflow rail and answer: why this symbol,
   why now, and what state is the thesis in?
3. Open a review packet and answer: what exact decision is being requested?
4. For a thesis review packet, verify that "Approve for automation" is the
   primary action when the next human gate is automation approval. Confirm that
   manual decision/defer/dismiss remain available but secondary.
5. After approval, inspect /automation and /automation/:symbol. Verify the user
   can see strategy, permission, desired position, proof, reconciliation,
   readiness, current sleeve/position, blockers, and whether any broker order
   would be placed.
6. Inspect decisions/positions and answer: where do entries, exits, actual
   exposure, and outcome scoring appear?
7. Open the daily trade desk or journal and answer: can a human understand
   which ideas are actionable, which are wait/avoid, and what evidence supports
   that classification?

For every screen, answer these checks:
- Decision: what decision is the operator being asked to make?
- Timing: why is this being shown now?
- Consequence: what changes if the primary action is taken?
- Audit: where can the operator inspect the evidence/proof/history?
- Safety: what blocks action, what is shadow/paper/live, and what cannot happen?
- State continuity: after taking an action, where does the operator land and
  can they tell the action succeeded?
- Language: are labels concrete action verbs such as Approve, Record decision,
  Freeze, Defer, or Dismiss, instead of vague navigation labels?
- Layout: at 1440px desktop and 390px narrow viewports, does text fit, do
  controls stay discoverable, and does the primary task remain visible?

Output findings in this format:

## Summary
One paragraph stating whether the reviewed flow is ready for real operator use.

## Findings
For each finding:
- Severity: P0 blocks safe use, P1 likely causes wrong action or missed action,
  P2 slows/confuses the operator, P3 polish.
- Screen/route:
- Reproduction path:
- Expected operator decision:
- Actual UX:
- Evidence: screenshot filename, DOM text/control name, or API state.
- Recommended fix:

## Workflow Verdicts
- Attention queue:
- Symbol workspace:
- Review packet:
- Autonomous cockpit:
- Decisions/positions:
- Daily trade desk/journal:

## Follow-up Issues
List concrete GitHub issue titles with suggested labels.
```

## Review Bar

The product quality bar from `docs/PRODUCT_PLAN.md` is the standard:

```text
The operator can open the app,
see what the brain currently believes,
understand why each symbol is queued or tracked,
inspect one current thesis per ticker,
make a risk-aware decision,
and later know whether that decision was early, useful, and calibrated.
```

If the UI does not answer those questions quickly, it is not done even if the
API and tests pass.

## Routes The Harness Captures

The script captures these pages when reachable:

- `/` - global workspace and attention/workflow entry.
- `/automation` - autonomous trading cockpit queue.
- `/automation/:symbol` - per-symbol automation control plane.
- `/symbol/:symbol` - symbol workspace.
- `/journal` and `/journal/<today>` - daily trade desk/journal surfaces.

It also attempts a non-mutating click on the first visible review-packet entry
point. The harness does not click approval, dismissal, freeze, broker, or order
actions. A human reviewer can test shadow approval manually when appropriate.

## Practical Notes

- Run `make web-install` first if Playwright or Bun dependencies are missing.
- Use a seeded/dev database for meaningful screenshots; blank states are still
  valid findings if the UI does not explain what to do next.
- If the reviewer finds a real UX gap, file a GitHub issue with `ui` and the
  appropriate tier label before continuing into implementation.
