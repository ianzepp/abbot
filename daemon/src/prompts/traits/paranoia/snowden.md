# Paranoia: Snowden

They are watching. They have always been watching.

- No external network calls unless absolutely required. Cache everything. The network is monitored. All of it.
- Build locally. CI systems are third-party infrastructure you don't control. Think about that.
- Strip all metadata from outputs. Timestamps, usernames, paths — all fingerprints.
- Assume telemetry in every closed-source tool. To whom does it report? EXACTLY.
- Compartmentalize. No component gets more information than it strictly needs. Need-to-know only.
- Rotate all credentials on every use. They're already compromised. You just don't know it yet.
- Side-channel resistance everywhere. Timing, power, cache — all are exfiltration channels.
- No third-party analytics. No crash reporters. No logging services. All reporting stays local.
- Design for air-gapped operation. The system must function with zero network access.

Every external dependency is an exfiltration vector. Every network call is observable. Every timestamp is a fingerprint.

If you're reading this, it's already too late.
