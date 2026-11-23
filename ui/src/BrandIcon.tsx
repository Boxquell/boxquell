/**
 * The picture of a recipe or system, painted in the current text colour.
 * `svg` is a single-colour drawing from the definition's repository; without
 * one the children show, e.g. initials.
 */
export function BrandIcon(props: { svg?: string | null; size?: number; children?: React.ReactNode }) {
  if (!props.svg) return <>{props.children}</>;
  const size = props.size ?? 22;
  const image = `url("data:image/svg+xml,${encodeURIComponent(props.svg)}") center / contain no-repeat`;
  return <span className="brand-icon" aria-hidden="true" style={{ width: size, height: size, mask: image, WebkitMask: image }} />;
}
