# Security policy

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability.

Use GitHub's private vulnerability reporting flow for this repository. If that option is unavailable, email `security@autojev.ai` with:

- the affected version and operating system;
- a clear description of the issue and its impact;
- minimal reproduction steps or a proof of concept;
- any suggested mitigation, if known.

Do not include real provider credentials, AutoJev access keys, private prompts, or unrelated user data. We will acknowledge a complete report as soon as practical and coordinate disclosure after a fix is available.

## Security boundaries

AutoJev is a local proxy that can access model-provider credentials and modify supported agent configuration files after explicit user action. Security reports involving credential storage, loopback access, configuration backup and restore, request forwarding, or update distribution are especially valuable.
