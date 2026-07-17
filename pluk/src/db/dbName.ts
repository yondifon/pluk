// Database-name validation and the pin rule for multi-database connections.
// Kept as a tiny pure module so the security logic can be unit-tested directly,
// independent of the driver/pool machinery.

// A database name is only ever passed as a connection-config value (never
// interpolated into SQL), but validate it anyway as defense in depth against a
// hostile identifier reaching a driver that might build a `USE`/qualified name.
const DB_NAME_RE = /^[A-Za-z0-9_$-]+$/;

export function isValidDatabaseName(name: string): boolean {
  return DB_NAME_RE.test(name) && name.length <= 128;
}

/**
 * Resolve the effective database for a driver from the connection's configured
 * database and an optional per-call override. Fails closed:
 *  - a hostile identifier is rejected;
 *  - a connection *pinned* to a database at setup can never be pointed at
 *    another (the override must match, or be absent).
 * Returns the database the driver should connect to (may be undefined = server
 * default for an unpinned connection with no override).
 */
export function resolveOverrideDatabase(configured: string | undefined, override?: string): string | undefined {
  if (!override) return configured;
  if (!isValidDatabaseName(override)) {
    throw new Error(`Invalid database name: ${override}`);
  }
  if (configured && configured !== override) {
    throw new Error(`Connection is locked to database "${configured}".`);
  }
  return override;
}
