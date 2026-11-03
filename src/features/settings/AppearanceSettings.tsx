import { RotateCcw } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { hintClass } from "@/components/ui";
import {
  DEFAULT_APPEARANCE,
  DENSITIES,
  FONT_SIZE,
  WIDTHS,
  readAppearance,
} from "@/lib/appearance";
import { cn } from "@/lib/cn";
import { COMMANDS } from "@/features/commands/registry";
import { formatShortcut } from "@/features/commands/shortcuts";

/** A group of mutually exclusive choices, announced as one radio group. */
function Choice<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: ReadonlyArray<{ value: T; label: string; hint?: string }>;
  onChange: (value: T) => void;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="flex flex-wrap gap-2">
      {options.map((option) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            onClick={() => onChange(option.value)}
            title={option.hint}
            className={cn(
              "focus-ring rounded-lg border px-3 py-2 text-[12.5px] transition",
              selected ? "bg-[var(--panel-2)]" : "hover:bg-[var(--panel-2)]",
            )}
            style={{ borderColor: selected ? "var(--accent)" : "var(--border)" }}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

function Row({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mt-5">
      <div className="mb-2 text-[12.5px] font-medium">{title}</div>
      {children}
    </div>
  );
}

/** Density, chat font size and transcript width. Changes apply immediately. */
export function AppearanceSettings() {
  const settings = useApp((state) => state.settings);
  const setAppearance = useApp((state) => state.setAppearance);
  const appearance = readAppearance(settings);
  const isDefault =
    appearance.density === DEFAULT_APPEARANCE.density &&
    appearance.fontSize === DEFAULT_APPEARANCE.fontSize &&
    appearance.chatWidth === DEFAULT_APPEARANCE.chatWidth;

  return (
    <div>
      <Row title="Density">
        <Choice
          label="Density"
          value={appearance.density}
          options={DENSITIES}
          onChange={(density) => void setAppearance({ density })}
        />
      </Row>

      <Row title="Chat text size">
        <div className="flex items-center gap-3">
          <input
            type="range"
            aria-label="Chat text size"
            aria-valuetext={`${appearance.fontSize} pixels`}
            min={FONT_SIZE.min}
            max={FONT_SIZE.max}
            step={FONT_SIZE.step}
            value={appearance.fontSize}
            onChange={(event) => void setAppearance({ fontSize: Number(event.target.value) })}
            className="focus-ring w-56 accent-[var(--accent)]"
          />
          <span className="w-12 text-[12.5px] tabular-nums" style={{ color: "var(--muted)" }}>
            {appearance.fontSize}px
          </span>
        </div>
        <p className={cn(hintClass, "mt-1.5")}>
          Sets the size of messages and tool activity in the chat. The rest of the app keeps its
          size.
        </p>
      </Row>

      <Row title="Chat width">
        <Choice
          label="Chat width"
          value={appearance.chatWidth}
          options={WIDTHS.map((width) => ({
            value: width.value,
            label: width.label,
            hint: width.px === null ? "Use the whole window" : `${width.px}px`,
          }))}
          onChange={(chatWidth) => void setAppearance({ chatWidth })}
        />
      </Row>

      <div className="mt-4">
        <Button
          size="sm"
          variant="ghost"
          icon={<RotateCcw size={13} />}
          disabled={isDefault}
          onClick={() => void setAppearance(DEFAULT_APPEARANCE)}
        >
          Reset to defaults
        </Button>
      </div>
    </div>
  );
}

/** Every keyboard shortcut, read from the same list the palette uses. */
export function ShortcutList() {
  const rows = COMMANDS.filter((command) => command.shortcut);
  return (
    <dl className="mt-1 grid grid-cols-[1fr_auto] gap-x-6 gap-y-1.5 text-[12.5px]">
      {rows.map((command) => (
        <div key={command.id} className="contents">
          <dt style={{ color: "var(--muted)" }}>{command.title.replace(/…$/, "")}</dt>
          <dd>
            <kbd
              className="rounded border px-1.5 py-0.5 font-mono text-[11px]"
              style={{ borderColor: "var(--border)" }}
            >
              {formatShortcut(command.shortcut!)}
            </kbd>
          </dd>
        </div>
      ))}
    </dl>
  );
}
