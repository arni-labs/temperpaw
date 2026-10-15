# Human Ask answers

Restore the user-authorized Foundry Record answer flow, which currently returns Temper HTTP403. Companion Foundry PR: https://github.com/arni-labs/foundry/pull/33. Existing Temper effort: 01a0ac15-ea5a-76b3-be6f-e2f2566e59de.

The configured human approver matches the registered operator identity. The current Ask policy omits that verified identity. Preserve native permission boundaries and do not substitute an agent credential.
