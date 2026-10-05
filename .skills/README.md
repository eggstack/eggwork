# Skills

Focused, task-shaped guidance for agents working in this repository. Each skill
answers one recurring question and points at the normative document rather than
restating it.

| Skill | Use it when |
|---|---|
| [pre-submit-gate](pre-submit-gate/SKILL.md) | Finishing a change, before committing, or asked "is this ready to submit?" |
| [cut-a-release](cut-a-release/SKILL.md) | Bumping a version, cutting or tagging a release, or diagnosing a failed release run |
| [architecture-routing](architecture-routing/SKILL.md) | Before reading source — which doc is normative, which code owns which boundary, and whether a suspected bug is already a known divergence |
| [isolation-and-trust](isolation-and-trust/SKILL.md) | Touching Landlock/cgroups, helper trust, capability advertisement, or admission |

## How these relate to the rest of the repo

- `AGENTS.md` is the always-loaded operating contract: commands, invariants, and
  style. Read it first.
- `architecture/` holds the normative design deep dives. The skills route you to
  the right one; the deep dive is what you cite.
- `plans/registry.md` is the active planning control surface. A skill will
  sometimes tell you to check it before opening work, because a plan may already
  name the thing you just noticed.

Skills are **derived** material. When a skill and `AGENTS.md` or an
`architecture/` deep dive disagree, the deep dive is right — fix the skill.
Skills carry no invariant of their own.
