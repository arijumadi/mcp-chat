import { CheckCircle2, Circle } from "lucide-react";
import { Button } from "@/components/Button";

export function SetupChecklist({
  providers,
  hasProviderKey,
  servers,
  onSettings,
  onServers,
}: {
  providers: number;
  hasProviderKey: boolean;
  servers: number;
  onSettings: () => void;
  onServers: () => void;
}) {
  const items = [
    {
      label: "Add a model provider",
      done: providers > 0,
      action: providers > 0 ? null : { label: "Add provider", onClick: onSettings },
    },
    {
      label: "Save an API key",
      done: hasProviderKey,
      action:
        providers > 0 && !hasProviderKey
          ? { label: "Open settings", onClick: onSettings }
          : null,
    },
    {
      label: "Connect an MCP server (optional)",
      done: servers > 0,
      action: servers > 0 ? null : { label: "Add MCP", onClick: onServers },
    },
  ];

  return (
    <div
      className="mt-7 rounded-xl border p-4 text-left"
      style={{ borderColor: "var(--border)", background: "var(--panel)" }}
    >
      <div className="text-[12.5px] font-medium">Finish setup</div>
      <div className="mt-2.5 grid gap-1.5">
        {items.map((item) => (
          <div key={item.label} className="flex items-center gap-2 text-[12px]">
            {item.done ? (
              <CheckCircle2 size={14} style={{ color: "var(--accent)" }} />
            ) : (
              <Circle size={14} style={{ color: "var(--faint)" }} />
            )}
            <span
              className="min-w-0 flex-1"
              style={{ color: item.done ? "var(--muted)" : "var(--fg)" }}
            >
              {item.label}
            </span>
            {item.action ? (
              <Button
                size="sm"
                variant="ghost"
                className="h-7 px-1.5 text-[10.5px]"
                onClick={item.action.onClick}
              >
                {item.action.label}
              </Button>
            ) : null}
          </div>
        ))}
      </div>
      <div className="mt-2.5 text-[10.5px]" style={{ color: "var(--faint)" }}>
        After starting a chat, choose the workspace from the composer.
      </div>
    </div>
  );
}
