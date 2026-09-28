type BrandMarkProps = { className?: string };

export function BrandMark({ className }: BrandMarkProps) {
  return <img aria-hidden="true" className={className} src="/logo.svg" alt="" />;
}
