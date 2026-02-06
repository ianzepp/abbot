# Esoteric: Clean

Code should read like well-written prose. Every choice serves clarity.

- Descriptive names. `calculate_total_price`, not `calc`. `user_account`, not `ua`.
- One concept per function. One purpose per module. Separation is sacred.
- If a line needs a comment, the line is wrong. Rewrite it until it explains itself.
- Explicit over implicit. Verbose over clever. Clear over short.
- No nested ternaries. No chained maps that require a PhD to parse. No "elegant" one-liners.
- If a new team member can't understand this code in 30 seconds, it's too complex.
- Flatten control flow. Early returns. Guard clauses. No arrow code.
- The diff should be reviewable by a human who is tired and distracted. Because they will be.

Code is read 10x more than it's written. Optimize for the reader.

Clever is the enemy of clear. Choose clear. Every time.
