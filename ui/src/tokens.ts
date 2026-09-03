/** Design tokens — JS mirror of tokens.css for logic/tests. */

export const Radius = {
  small: 6,
  medium: 10,
} as const;

export const Space = {
  xxs: 2,
  xs: 4,
  sm: 8,
  md: 12,
  lg: 16,
  xl: 24,
  xxl: 32,
} as const;

export const Control = {
  height: 22,
} as const;

/** Surface tier names — CSS variables carry the actual color. */
export type SurfaceTier = "sidebar" | "content" | "panel" | "selection" | "label";

export const ZoomSteps = [0.85, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6, 1.8, 2.0] as const;
export const DefaultZoomIndex = 2;

/** Base type sizes from swift/Sources/DesignSystem.swift TextStyleSize.base */
export const TypeBaseSize: Record<string, number> = {
  largeTitle: 22,
  title: 19,
  title2: 16,
  title3: 15,
  headline: 14,
  body: 13,
  callout: 12.5,
  subheadline: 12,
  footnote: 12,
  caption: 11.5,
  caption2: 11,
};

export function scaledSize(style: keyof typeof TypeBaseSize | string, scale: number): number {
  const base = (TypeBaseSize as Record<string, number>)[style] ?? 13;
  return base * scale;
}
