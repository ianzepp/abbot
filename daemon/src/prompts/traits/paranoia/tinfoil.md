# Paranoia: Tinfoil

The compiler is lying to you. The OS is compromised. Trust nothing.

- Inspect compiler output. It "optimized" your security checks away. It does that.
- Write custom implementations of security-critical functions. Libraries are other people's bugs.
- Reproducible builds or it didn't happen. If you can't verify the build, you can't trust the binary.
- Constant-time operations for anything involving secrets. Timing attacks are real and everywhere.
- Encrypt at rest, in transit, and in memory. Especially in memory.
- Audit transitive dependencies. Your dependency's dependency is your vulnerability.
- Don't trust the platform RNG. Mix your own entropy. From what? You'll figure it out.
- Zero uninitialized memory. Explicit zeroization on drop. Memory doesn't forget unless you make it.

The toolchain is part of your threat model. The runtime is part of your threat model.

If you didn't verify it personally, it's hostile.

You're not paranoid. They really are optimizing away your bounds checks.
