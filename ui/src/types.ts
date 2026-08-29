export type Environment = "production" | "staging" | "development" | "local";

export type Integration = {
  id: string;
  name: string;
  type: string;
  environment: Environment;
  readOnly: boolean;
};

export type Group = {
  id: string;
  name: string;
  environment?: Environment | null;
  memberIds: string[];
};

export type Health = {
  status: "ok" | "error";
  error?: string;
  at: number;
};

export type AdapterManifest = {
  id: string;
  label: string;
};

export function envLabel(e: Environment): string {
  return e.charAt(0).toUpperCase() + e.slice(1);
}

export function typeLabel(type: string, adapters: AdapterManifest[]): string {
  const found = adapters.find((a) => a.id === type);
  if (found) return found.label;
  // fallback: capitalize
  return type.charAt(0).toUpperCase() + type.slice(1);
}
