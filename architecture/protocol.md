# Protocol layering (pre-wire)

The core data model is protocol-neutral. Later control-plane work will define versioned operations over Eggstack transport with transport-derived principal identity. Large blobs will use a streaming data path rather than JSON expansion. The exact public wire schema is not frozen here.

See the [control-plane roadmap](../plans/subsystems/control-plane-protocol-roadmap.md) and [ADR-0002](../plans/adrs/ADR-0002-eggstack-transport-and-mtls.md).
