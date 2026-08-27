/**
 * Adapter glyphs — per-adapter colours and logo fallback.
 * Colours copied from swift/Sources/ContentView.swift AdapterStyle.color
 */

export const adapterColors: Record<string, string> = {
  postgres: "#4d75a8", // 0.30,0.46,0.66
  mysql: "#c78c33", // 0.78,0.55,0.20
  sqlite: "#73808f", // 0.45,0.50,0.56
  linear: "#5e6ad2", // 0.37,0.42,0.82 approx Linear indigo
  sentry: "#7d6bc7", // 0.49,0.42,0.78
  ssh: "#458c73", // 0.27,0.55,0.45
  "github-cli": "#38404d", // 0.22,0.25,0.30
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
    case "sqlite":
      return "LT";
    default:
      return type.slice(0, 2).toUpperCase();
  }
}

export function adapterSymbol(type: string): string | null {
  if (type === "ssh") return "⌥"; // terminal symbol placeholder; CSS will style
  return null;
}

/**
 * Render glyph element: tries image if available, else symbol, else abbrev.
 * Caller should check if an image URL exists; we expose a helper.
 */
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

  const symbol = adapterSymbol(type);
  if (symbol) {
    wrap.textContent = symbol;
    wrap.style.fontSize = `${size * 0.9}px`;
    return wrap;
  }

  // Abbrev fallback (logos would be <img> here if bundled; none bundled today)
  wrap.textContent = adapterAbbrev(type);
  wrap.style.fontFamily = "var(--font-mono)";
  wrap.title = type;
  return wrap;
}
