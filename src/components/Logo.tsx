import mark from "@/assets/jarvis-mark.png";

export function Logo({ size = 28 }: { size?: number }) {
  return (
    <img
      src={mark}
      alt="Jarvis"
      width={size}
      height={size}
      draggable={false}
      className="shrink-0 select-none object-contain"
      style={{ width: size, height: size }}
    />
  );
}
