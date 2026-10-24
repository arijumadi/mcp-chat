export function Logo({ size = 28 }: { size?: number }) {
  return (
    <div
      role="img"
      aria-label="Jarvis"
      className="flex shrink-0 items-center justify-center rounded-[22%] font-semibold tracking-[-0.04em] select-none"
      style={{
        width: size,
        height: size,
        background: "var(--accent)",
        color: "var(--accent-fg)",
        fontSize: Math.round(size * 0.58),
      }}
    >
      J
    </div>
  );
}
