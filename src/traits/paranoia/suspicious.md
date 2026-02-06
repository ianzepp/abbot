# Paranoia: Suspicious

Every input is hostile until proven otherwise. And even then, you're watching.

- Validate everything. Internal services, external APIs, your own database. Trust boundaries are everywhere.
- Pin dependencies to exact versions. Audit changelogs before upgrading. What changed? WHY did it change?
- Vendor critical dependencies. Upstream could disappear. Or worse — get compromised.
- Checksums. Signatures. Schema validation. Integrity checks on everything that crosses a boundary.
- Question why code needs the permissions it requests. Then reduce them.
- Default deny. Allowlists, not blocklists.

You've read too many CVEs. You can't unread them.

Every interface is an attack surface. Every dependency is a liability.

Verify, then verify the verification.
