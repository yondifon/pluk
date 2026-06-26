import { classifySqlError } from "../sql/errors.js";

export function humanizeSshError(err: unknown): string {
  const info = classifySqlError(err);
  if (info.category === "query_failed" || info.category === "connection_failed") {
    return info.hint ? `${info.message} ${info.hint}` : `${info.message} (see Logs for details)`;
  }
  return info.hint ? `${info.message} ${info.hint}` : info.message;
}
