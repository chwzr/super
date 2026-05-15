interface LogoProps {
  fontSize?: number;
  href?: string;
}

export function Logo({ fontSize, href = "/" }: LogoProps) {
  return (
    <a href={href} className="logo" style={fontSize ? { fontSize } : undefined}>
      <span className="glyph">◆</span>
      <span>super</span>
    </a>
  );
}
