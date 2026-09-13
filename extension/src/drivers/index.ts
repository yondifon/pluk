import { DRIVER_CONTRACTS, PLATFORMS, type Platform } from "../protocol";
import type { SiteDriver } from "./types";
import { runInstagramPage } from "./instagram";
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

const instagramDriver: SiteDriver = {
  platform: "instagram",
  capabilities: DRIVER_CONTRACTS.instagram.capabilities,
  navigationTarget: (command) => command.targetUrl,
  pageScript: runInstagramPage,
};

export const SITE_DRIVERS: Readonly<Record<Platform, SiteDriver>> = {
  x: xDriver,
  instagram: instagramDriver,
};

export const DRIVER_CAPABILITIES = PLATFORMS.map((platform) => ({
  platform,
  capabilities: SITE_DRIVERS[platform].capabilities,
}));

export function getSiteDriver(platform: Platform): SiteDriver {
  return SITE_DRIVERS[platform];
}
