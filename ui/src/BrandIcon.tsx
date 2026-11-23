/**
 * The picture of a recipe or system, painted in the current text colour.
 * `svg` is a single-colour drawing from the definition's repository; without
 * one the children show, e.g. initials.
 */
export function BrandIcon(props: { svg?: string | null; size?: number; children?: React.ReactNode }) {
  if (!props.svg) return <>{props.children}</>;
  const size = props.size ?? 24;
  const image = `url("data:image/svg+xml,${encodeURIComponent(props.svg)}") center / contain no-repeat`;
  return <span className="brand-icon" aria-hidden="true" style={{ width: size, height: size, mask: image, WebkitMask: image }} />;
}

/**
 * Colours for the square behind a picture: the brand's colour with white or
 * black on it, whichever reads better. Without a valid colour, nothing.
 */
export function brandColors(color?: string | null): React.CSSProperties | undefined {
  if (!color || !/^#[0-9a-f]{6}$/i.test(color)) return undefined;
  const [r, g, b] = [1, 3, 5].map((at) => parseInt(color.slice(at, at + 2), 16) / 255).map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  const luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  return { background: color, color: luminance > 0.45 ? "#000" : "#fff" };
}
