import { classifySqlError } from "../sql/errors.js";
import { CommandTimeoutError, MAX_COMMAND_TIMEOUT_S } from "./client.js";

export function humanizeSshError(err: unknown): string {
  if (err instanceof CommandTimeoutError) {
    return `${err.message}. The command exceeded the timeout — retry with a higher \`timeout\` (up to ${MAX_COMMAND_TIMEOUT_S} seconds).`;
  }
  const info = classifySqlError(err);
  if (info.category === "query_failed" || info.category === "connection_failed") {
    return info.hint ? `${info.message} ${info.hint}` : `${info.message} (see Logs for details)`;
  }
  return info.hint ? `${info.message} ${info.hint}` : info.message;
}
