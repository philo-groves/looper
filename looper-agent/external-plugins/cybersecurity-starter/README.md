# cybersecurity-starter

Starter external plugin for cybersecurity workflows.

Install from chat:

`/plugin add looper-agent/external-plugins/cybersecurity-starter`

Primary actuators:

- `cyber_surface_map`
- `cyber_endpoint_inventory`
- `cyber_tech_fingerprint`
- `cyber_triage`
- `cyber_hypothesis_generate`
- `cyber_validate_finding`
- `cyber_report_outline`
- `cyber_report_draft`
- `cyber_report_evidence_pack`
- `cyber_report_exec_summary`

Terminal shortcuts (after install):

- `/cyber-surface <target>`
- `/cyber-triage <target>`
- `/cyber-validate <target>`
- `/cyber-report <target>`

## Operating Mode

This starter pack is passive-first by default:

- Recon and fingerprinting are inference-only.
- Validation is `passive_validation_only` and requires evidence/repro metadata.
- No active exploit behavior is included in this pack.

This pack is intentionally lightweight and safe-by-default; it gives a scaffold you can extend with deeper recon, validation, and reporting behaviors.
