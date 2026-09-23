# Operations M002 — Blocker Record

Status: blocked; implementation not started beyond dependency/API qualification.

Source implementation plan: `plans/implementation/operations-distribution/002-eggup-deployment-and-service-integration.md`

Roadmap: `plans/subsystems/operations-distribution-roadmap.md`

Reviewed Eggwork head before this record: `b3b5459a6077f59278fa1694a65f8c29a76000ef`.

## Finding

The plan requires Eggwork to restart an updated service, perform a bounded post-start health/version check, then invoke Eggup rollback if that check fails. The reviewed Eggup main revision is `66813b3b94de3a9b2f270e0000dc339ef6f0b478`. The published `eggup-core 0.1.0` and `eggup-service 0.1.0` do not provide the reviewed manager adapters, so the plan permits an exact immutable Git revision for integration qualification. That revision has the adapters, but inspection of `crates/eggup-core/src/transaction.rs` shows that successful `ValidatedTransaction::commit` cleans its backup set; the documented post-commit `KeepInstalled | RollBack` policy is reserved for a future boundary and is not implemented.

Consequently, the required health failure occurs after Eggup has discarded the backups needed to restore the prior installation. Eggwork-side file restoration or deletion would implement filesystem transaction mechanics outside Eggup and violate the plan's ownership boundary. No safe implementation can satisfy the acceptance criteria against this interface.

## Qualification and implementation evidence

| Area | Result |
|---|---|
| Eggup publication/pinning | Published 0.1.0 lacks the adapters; exact Git revision `66813b3b94de3a9b2f270e0000dc339ef6f0b478` has them. No Eggup dependency pin was added to Eggwork. |
| Transaction/rollback | Blocked: no retained backup or post-commit rollback API. |
| Install artifact matrix | Not implemented or qualified. |
| Service ownership/adapters | Not integrated or qualified in Eggwork. |
| Drain/update orchestration | Not implemented. |
| Helper compatibility | Not implemented. |
| Hosted manager matrix | None claimed. |
| Producer packaging | Remains outside M002 and owned by Eggpack. |
| Tests | No M002 implementation tests run; no runtime qualification claimed. |

## Required unblock evidence

Resume M002 only when one of these is true:

1. Eggup provides an API that safely retains/restores the prior filesystem transaction through post-start health validation and exposes rollback at that boundary; or
2. The architecture owner revises the transaction ownership and rollback requirements, with an explicit safe replacement design.

Do not work around this blocker by implementing file backup, restoration, or deletion in Eggwork. The dependency publication/pinning decision can then be made again against the available Eggup version and reproducibility policy.

## Downstream disposition

- Operations M003 remains blocked on Eggpack producer interfaces, independently of this blocker.
- Operations M004 remains blocked on M002 and M003.
- Security M004 remains blocked on Operations M002, despite Foundation M003 and Control Plane M003 now being closed.
- CodeGG M001 is independent of Operations M002 and remains ready for its own repository planning process.

Final disposition: M002 is formally recorded as blocked, not closed. No M002 acceptance criteria are claimed complete.
