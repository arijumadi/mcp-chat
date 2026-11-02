import { useState } from "react";
import { Bell } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { hintClass } from "@/components/ui";
import { call } from "@/lib/ipc";
import { notificationsEnabled, sendTestNotification } from "@/lib/notify";

/** Desktop notifications for turns that finish or need you while you are elsewhere. */
export function NotificationSettings() {
  const settings = useApp((state) => state.settings);
  const refreshSettings = useApp((state) => state.refreshSettings);
  const toast = useApp((state) => state.toast);
  const [testing, setTesting] = useState(false);
  const enabled = notificationsEnabled(settings);

  const toggle = async () => {
    const next = !enabled;
    // Update at once; the setting is saved in the background.
    useApp.setState((state) => ({
      settings: { ...state.settings, "notifications.enabled": String(next) },
    }));
    try {
      await call("set_setting", { key: "notifications.enabled", value: String(next) });
    } catch (error) {
      toast(String(error), "error");
      await refreshSettings();
    }
  };

  const test = async () => {
    setTesting(true);
    const result = await sendTestNotification();
    setTesting(false);
    if (result === "sent") toast("Test notification sent.", "info");
    else if (result === "blocked") {
      toast("Notifications are blocked. Allow them for Jarvis in your system settings.", "error");
    } else toast("Notifications are not available here.", "error");
  };

  return (
    <div>
      <label className="flex cursor-pointer items-start gap-3 text-[13px]">
        <input
          type="checkbox"
          checked={enabled}
          onChange={() => void toggle()}
          className="focus-ring mt-0.5 h-4 w-4 accent-[var(--accent)]"
        />
        <span>
          <span className="font-medium">Notify me when I am away</span>
          <span className={`${hintClass} mt-0.5 block`}>
            A desktop notification when a reply finishes or a tool needs your approval or your
            answer, while Jarvis is in the background or another chat is open. It shows the chat
            title only, never your messages or commands.
          </span>
        </span>
      </label>
      <div className="mt-3">
        <Button size="sm" variant="secondary" icon={<Bell size={13} />} loading={testing} onClick={() => void test()}>
          Send a test notification
        </Button>
      </div>
    </div>
  );
}
