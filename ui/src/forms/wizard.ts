import type { AdapterManifest } from "./catalog.ts";

export type WizardStepKind = "type" | "name" | "connect" | "tools" | "commands" | "install";

export type WizardMode = "create" | "edit";

/**
 * The screens a create or edit flow walks through, in order: whether "type"
 * opens the flow and whether "commands" sits between tools and install.
 */
export function wizardSteps(manifest: AdapterManifest | undefined, mode: WizardMode): WizardStepKind[] {
  const steps: WizardStepKind[] = [];
  if (mode === "create") steps.push("type");
  steps.push("name", "connect", "tools");
  if (manifest?.runsCommands) steps.push("commands");
  steps.push("install");
  return steps;
}
