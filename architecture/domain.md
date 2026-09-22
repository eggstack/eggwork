# Domain model

The core crate defines bounded newtype identities and protocol-neutral execution contracts. An execution is one validated argv command, optional relative workspace cwd, explicit environment and stdin policies, output bounds, optional declared outputs, and resource/isolation/network requirements.

IDs are opaque values. Generation orders control ownership; event sequence orders observations. Blob identity is lowercase SHA-256 of bytes. Requests carry an explicit schema version. Canonical request digesting is version-prefixed and independent of JSON map ordering because core request fields are ordered structs; future wire compatibility remains subject to the relevant implementation plans.

The normative vocabulary and invariants are in [the domain model](../plans/001-terminology-and-domain-model.md).
