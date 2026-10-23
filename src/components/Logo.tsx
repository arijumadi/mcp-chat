export function Logo({ size = 28 }: { size?: number }) {
  return (
    <img
      src="/icon.png"
      alt=""
      width={size}
      height={size}
      className="shrink-0 rounded-[22%]"
      draggable={false}
    />
  );
}
