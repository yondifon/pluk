import { createIcon } from "./icon";
import { adapterLogo } from "./adapterLogo";

export const adapterColors: Record<string, string> = {
  postgres: "#4d75a8", // 0.30,0.46,0.66
  mysql: "#c78c33", // 0.78,0.55,0.20
  mssql: "#6a7d8f",
  sqlite: "#73808f", // 0.45,0.50,0.56
  linear: "#5e6ad2", // 0.37,0.42,0.82 approx Linear indigo
  sentry: "#7d6bc7", // 0.49,0.42,0.78
  ssh: "#458c73", // 0.27,0.55,0.45
  "github-cli": "#6e7681",
  redis: "#c7402e", // 0.78,0.25,0.18
  slack: "#752e73", // 0.46,0.18,0.45
  spark: "#d95438", // 0.85,0.33,0.22
};

export function adapterColor(type: string): string {
  return adapterColors[type] ?? "#66687f";
}

export function adapterAbbrev(type: string): string {
  switch (type) {
    case "postgres":
      return "PG";
    case "mysql":
      return "MY";
    case "mssql":
      return "MS";
    case "sqlite":
      return "LT";
    default:
      return type.slice(0, 2).toUpperCase();
  }
}

export function glyphElement(type: string, size = 12): HTMLElement {
  const wrap = document.createElement("span");
  wrap.className = "adapter-glyph";
  wrap.style.width = `${size}px`;
  wrap.style.height = `${size}px`;
  wrap.style.display = "inline-flex";
  wrap.style.alignItems = "center";
  wrap.style.justifyContent = "center";
  wrap.style.flexShrink = "0";
  wrap.style.borderRadius = `${size * 0.25}px`;
  wrap.style.fontSize = `${size * 0.8}px`;
  wrap.style.fontWeight = "600";
  wrap.style.color = adapterColor(type);

  if (type === "ssh") {
    wrap.appendChild(createIcon("terminal", { size }));
    return wrap;
  }

  const logo = adapterLogo(type);
  if (logo) {
    wrap.title = type;
    wrap.appendChild(logo);
    return wrap;
  }

  wrap.textContent = adapterAbbrev(type);
  wrap.style.fontFamily = "var(--font-mono)";
  wrap.title = type;
  return wrap;
}

export function hexToRgba(hex: string, alpha: number): string {
  const h = hex.replace("#", "");
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return `rgba(${r},${g},${b},${alpha})`;
}

/** The large square mark used in detail headers and the type chooser. */
export function typeBadge(type: string, label: string): HTMLElement {
  const badge = document.createElement("div");
  badge.className = "type-badge";
  badge.setAttribute("aria-hidden", "true");
  badge.style.color = adapterColor(type);

  if (type === "ssh") {
    badge.style.background = hexToRgba(adapterColor(type), 0.14);
    badge.appendChild(createIcon("terminal", { size: 20 }));
    return badge;
  }

  const logo = adapterLogo(type);
  if (logo) {
    badge.style.background = hexToRgba(adapterColor(type), 0.14);
    badge.appendChild(logo);
    return badge;
  }

  badge.textContent = type === "mssql" ? adapterAbbrev(type) : label.slice(0, 2).toUpperCase();
  return badge;
}
