const MINUTE = 60_000
const HOUR = 60 * MINUTE

export const Policy = {
  /** A browser created the session and nobody has paired yet. */
  openTtlMs: 15 * MINUTE,
  /** Sliding window once an agent has paired; agent attach and text frames renew it. */
  pairedTtlMs: 2 * HOUR,
  /** Absolute ceiling from creation, however active. */
  hardTtlMs: 6 * HOUR,
  /** How long a session waits for its visitor to return after the last browser socket goes. */
  browserGraceMs: 5 * MINUTE,
} as const
