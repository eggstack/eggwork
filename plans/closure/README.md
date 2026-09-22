# Eggwork Closure Records

Closure records are implementation evidence, not plans.

A milestone is not closed because code exists. It is closed only when its implementation plan's acceptance criteria have been verified and recorded.

## Required contents

A closure record should include:

- source implementation plan and subsystem roadmap;
- reviewed repository head and implementation commits;
- executive finding;
- requirement-to-evidence matrix;
- exact focused and broader verification performed;
- cancellation/restart/contention evidence where applicable;
- security and negative-test evidence;
- compatibility/migration review;
- platform/feature qualification matrix actually exercised;
- documentation status;
- unresolved findings ranked by severity;
- final disposition;
- registry/roadmap updates.

## Evidence discipline

- Do not claim tests were run when they were only inspected.
- Do not qualify a platform based only on cross-compilation.
- Preserve historical closure records if later findings appear; add corrective work instead of rewriting history.
- A blocked external interface is a legitimate closure blocker and must be recorded truthfully.
