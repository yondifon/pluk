export type IconName =
  | "add"
  | "check"
  | "chevron-right"
  | "close"
  | "copy"
  | "edit"
  | "error"
  | "filter"
  | "group"
  | "group-add"
  | "info"
  | "lock"
  | "more"
  | "refresh"
  | "search"
  | "spinner"
  | "terminal"
  | "trash"
  | "tray";

type IconOptions = {
  size?: number;
  label?: string;
};

const paths: Record<IconName, string[]> = {
  add: ["M12 5v14", "M5 12h14"],
  check: ["m5 12 4 4L19 6"],
  "chevron-right": ["m9 5 7 7-7 7"],
  close: ["m6 6 12 12", "M18 6 6 18"],
  copy: ["M9 9h10v11H9z", "M15 6H5v11"],
  edit: ["m4 20 1-4.5L16.6 3.9a2.2 2.2 0 0 1 3.1 3.1L8.5 19 4 20Z", "m14.5 6 3.5 3.5"],
  error: ["M12 3 21 20H3L12 3Z", "M12 9v4", "M12 17h.01"],
  filter: ["M4 7h16", "M7 12h10", "M10 17h4"],
  group: ["M5 8.5h11v10H5z", "M8 5h11v10H8z"],
  "group-add": ["M4 7h11v11H4z", "M17 12v7", "m13.5 15.5 7 0"],
  info: ["M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Z", "M12 11.5v5", "M12 8h.01"],
  lock: ["M6 11h12v9H6z", "M8 11V8a4 4 0 0 1 8 0v3"],
  more: ["M6 12h.01", "M12 12h.01", "M18 12h.01"],
  refresh: ["M19 8V4m0 0h-4m4 0-4.5 4.5", "M20 14a8 8 0 1 1-2-5.3"],
  search: ["m20 20-4.5-4.5", "M16 10a6 6 0 1 1-12 0 6 6 0 0 1 12 0Z"],
  spinner: ["M12 4a8 8 0 1 0 8 8"],
  terminal: ["M4 5h16v14H4z", "m7 9 3 3-3 3", "M12 15h5"],
  trash: ["M5 7h14", "M9.5 7V4.5h5V7", "m7 7 1 12.5h8L17 7"],
  tray: ["M4 5h16v14H4z", "M4 14h5l1.5 2h3L15 14h5"],
};

export function createIcon(name: IconName, options: IconOptions = {}): SVGSVGElement {
  const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  icon.classList.add("icon", `icon-${name}`);
  icon.setAttribute("viewBox", "0 0 24 24");
  icon.setAttribute("fill", "none");
  icon.setAttribute("stroke", "currentColor");
  icon.setAttribute("stroke-width", "1.5");
  icon.setAttribute("stroke-linecap", "round");
  icon.setAttribute("stroke-linejoin", "round");
  icon.setAttribute("data-icon", name);
  if (options.size != null) icon.style.setProperty("--icon-size", `${options.size}px`);

  if (options.label) {
    icon.setAttribute("role", "img");
    icon.setAttribute("aria-label", options.label);
  } else {
    icon.setAttribute("aria-hidden", "true");
  }

  for (const d of paths[name]) {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", d);
    icon.appendChild(path);
  }

  return icon;
}
