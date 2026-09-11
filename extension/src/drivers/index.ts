import { DRIVER_CONTRACTS, PLATFORMS, type Platform } from "../protocol";
import type { SiteDriver } from "./types";
import { runXPage } from "./x";

const xDriver: SiteDriver = {
  platform: "x",
  capabilities: DRIVER_CONTRACTS.x.capabilities,
  navigationTarget: (command) =>
    command.action === "read_trends"
      ? "https://x.com/explore"
      : command.targetUrl,
  pageScript: runXPage,
};

export const SITE_DRIVERS: Readonly<Record<Platform, SiteDriver>> = {
  x: xDriver,
};

export const DRIVER_CAPABILITIES = PLATFORMS.map((platform) => ({
  platform,
  capabilities: SITE_DRIVERS[platform].capabilities,
}));

export function getSiteDriver(platform: Platform): SiteDriver {
  return SITE_DRIVERS[platform];
}
