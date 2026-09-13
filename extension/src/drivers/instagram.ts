import type { DriverPageResult, DriverScriptOptions } from "./types";

// Instagram's page logic has not been implemented yet: every command comes
// back unsupported until the site driver is filled in.
export function runInstagramPage(
  _options: DriverScriptOptions,
): DriverPageResult | Promise<DriverPageResult> {
  return {
    state: "unsupported",
    message: "The Instagram site driver is not implemented yet.",
  };
}
