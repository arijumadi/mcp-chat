import { lazy, Suspense, useEffect, useState } from "react";
import { useApp } from "@/app/store";
import { Sidebar } from "@/components/Sidebar";
import { QuickSwitcher } from "@/components/QuickSwitcher";
import { Toasts } from "@/components/Toasts";
import { ChatView } from "@/features/chat/ChatView";
import { ApprovalModal } from "@/features/chat/ApprovalModal";
import { InputRequestModal } from "@/features/chat/InputRequestModal";
const SettingsView = lazy(() =>
  import("@/features/settings/SettingsView").then((module) => ({
    default: module.SettingsView,
  })),
);
const ServersView = lazy(() =>
  import("@/features/servers/ServersView").then((module) => ({
    default: module.ServersView,
  })),
);

export default function App() {
  const booted = useApp((state) => state.booted);
  const view = useApp((state) => state.view);
  const sidebarOpen = useApp((state) => state.sidebarOpen);
  const init = useApp((state) => state.init);
  const newChat = useApp((state) => state.newChat);
  const [quickOpen, setQuickOpen] = useState(false);

  useEffect(() => {
    void init();
  }, [init]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setQuickOpen(true);
      }
      if (mod && event.key.toLowerCase() === "n") {
        event.preventDefault();
        void newChat();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [newChat]);

  if (!booted) return <BootSkeleton />;

  return (
    <div className="flex h-dvh overflow-hidden" style={{ background: "var(--bg)" }}>
      {sidebarOpen ? <Sidebar /> : null}
      <main className="flex min-w-0 flex-1 flex-col">
        {view === "chat" ? (
          <ChatView />
        ) : (
          <Suspense fallback={<ViewSkeleton />}>
            {view === "settings" ? <SettingsView /> : <ServersView />}
          </Suspense>
        )}
      </main>
      <QuickSwitcher open={quickOpen} onClose={() => setQuickOpen(false)} />
      <ApprovalModal />
      <InputRequestModal />
      <Toasts />
    </div>
  );
}

function ViewSkeleton() {
  return (
    <div className="mx-auto w-full max-w-[820px] px-6 py-7">
      <div className="shimmer mb-2 h-5 w-32 rounded" />
      <div className="shimmer mb-6 h-3 w-72 rounded" />
      <div className="shimmer mb-3 h-24 w-full rounded-xl" />
      <div className="shimmer h-24 w-full rounded-xl" />
    </div>
  );
}

function BootSkeleton() {
  return (
    <div className="flex h-dvh overflow-hidden" style={{ background: "var(--bg)" }}>
      <div
        className="hidden w-[260px] shrink-0 flex-col gap-2 border-r p-3 sm:flex"
        style={{ borderColor: "var(--border)", background: "var(--panel)" }}
      >
        <div className="mb-3 flex items-center gap-2">
          <div className="shimmer h-6 w-6 rounded-lg" />
          <div className="shimmer h-3 w-24 rounded" />
        </div>
        <div className="shimmer h-8 w-full rounded-lg" />
        <div className="shimmer h-8 w-full rounded-lg" />
        {Array.from({ length: 7 }).map((_, index) => (
          <div key={index} className="shimmer h-7 w-full rounded-lg" />
        ))}
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <div
          className="flex h-14 items-center gap-2 border-b px-4"
          style={{ borderColor: "var(--border)" }}
        >
          <div className="shimmer h-4 w-44 rounded" />
          <div className="shimmer ml-auto h-7 w-40 rounded-lg" />
        </div>
        <div className="mx-auto w-full max-w-[760px] flex-1 space-y-4 px-6 py-8">
          <div className="shimmer h-12 w-2/3 rounded-2xl" />
          <div className="shimmer h-4 w-full rounded" />
          <div className="shimmer h-4 w-11/12 rounded" />
          <div className="shimmer h-4 w-4/5 rounded" />
          <div className="shimmer h-20 w-full rounded-xl" />
          <div className="shimmer h-4 w-3/4 rounded" />
        </div>
        <div className="px-4 pb-4">
          <div className="shimmer mx-auto h-14 w-full max-w-[760px] rounded-2xl" />
        </div>
      </div>
    </div>
  );
}
