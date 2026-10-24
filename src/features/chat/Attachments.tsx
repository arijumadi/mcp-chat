import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { FileText, Image as ImageIcon, Paperclip, X } from "lucide-react";
import { Modal } from "@/components/Modal";
import { cn } from "@/lib/cn";
import { formatBytes } from "@/lib/format";
import type { Attachment, Part } from "@/lib/types";
import { parsePayload } from "@/app/store";

export function attachmentFromPart(part: Part): Attachment | null {
  if (part.kind !== "attachment") return null;
  const payload = parsePayload(part);
  const name = typeof payload.name === "string" ? payload.name : "";
  const kind =
    typeof payload.fileKind === "string"
      ? payload.fileKind
      : typeof payload.kind === "string"
        ? payload.kind
        : "file";
  const path = typeof payload.path === "string" ? payload.path : "";
  if (!name || !path) return null;
  return {
    id: typeof payload.id === "string" ? payload.id : part.id,
    name,
    mime: typeof payload.mime === "string" ? payload.mime : "application/octet-stream",
    size: typeof payload.size === "number" ? payload.size : 0,
    kind,
    path,
    text: typeof payload.text === "string" ? payload.text : null,
    truncated: Boolean(payload.truncated),
  };
}

export function attachmentsFromParts(parts: Part[]): Attachment[] {
  return parts
    .map(attachmentFromPart)
    .filter((attachment): attachment is Attachment => attachment !== null);
}

function assetUrl(path: string): string {
  if (!path) return "";
  if (window.__MCP_CHAT_MOCK__) return "";
  if (!("__TAURI_INTERNALS__" in window)) return "";
  try {
    return convertFileSrc(path);
  } catch {
    return "";
  }
}

function AttachmentIcon({ attachment }: { attachment: Attachment }) {
  if (attachment.kind === "image") return <ImageIcon size={14} />;
  if (attachment.kind === "text") return <FileText size={14} />;
  return <Paperclip size={14} />;
}

export function AttachmentGrid({
  attachments,
  compact = false,
  onRemove,
  align = "start",
}: {
  attachments: Attachment[];
  compact?: boolean;
  onRemove?: (id: string) => void;
  align?: "start" | "end";
}) {
  const [preview, setPreview] = useState<Attachment | null>(null);
  if (attachments.length === 0) return null;

  return (
    <>
      <div
        className={cn(
          "flex flex-wrap gap-1.5",
          align === "end" ? "justify-end" : "justify-start",
        )}
      >
        {attachments.map((attachment) => (
          <AttachmentTile
            key={attachment.id}
            attachment={attachment}
            compact={compact}
            onRemove={onRemove}
            onPreview={() => setPreview(attachment)}
          />
        ))}
      </div>
      <Modal
        open={preview !== null}
        onClose={() => setPreview(null)}
        title={preview?.name ?? "Attachment"}
        subtitle={
          preview
            ? `${preview.mime} · ${formatBytes(preview.size)}`
            : undefined
        }
        width="xl"
      >
        {preview?.kind === "image" ? (
          <div>
            {assetUrl(preview.path) ? (
              <img
                src={assetUrl(preview.path)}
                alt={preview.name}
                className="mx-auto max-h-[70vh] max-w-full rounded-lg object-contain"
              />
            ) : (
              <p className="text-[13px]" style={{ color: "var(--muted)" }}>
                Preview is unavailable. The image is stored locally.
              </p>
            )}
          </div>
        ) : preview?.kind === "text" ? (
          <div>
            <pre
              className="max-h-[65vh] overflow-auto rounded-lg border p-3 text-[11.5px] whitespace-pre-wrap"
              style={{
                borderColor: "var(--border)",
                background: "var(--panel-2)",
              }}
            >
              {preview.text ?? ""}
            </pre>
            {preview.truncated ? (
              <p className="mt-2 text-[11px]" style={{ color: "var(--faint)" }}>
                Preview is truncated.
              </p>
            ) : null}
          </div>
        ) : preview ? (
          <p className="text-[13px]" style={{ color: "var(--muted)" }}>
            This is a binary file. Its metadata is included in the message; the
            binary content is not sent to the model.
          </p>
        ) : null}
      </Modal>
    </>
  );
}

function AttachmentTile({
  attachment,
  compact,
  onRemove,
  onPreview,
}: {
  attachment: Attachment;
  compact: boolean;
  onRemove?: (id: string) => void;
  onPreview: () => void;
}) {
  const [failed, setFailed] = useState(false);
  const isImage = attachment.kind === "image";
  const src = isImage ? assetUrl(attachment.path) : "";
  const showImage = isImage && src && !failed;

  if (showImage) {
    return (
      <div className="group/attachment relative">
        <button
          type="button"
          onClick={onPreview}
          className="focus-ring block overflow-hidden rounded-lg border transition hover:brightness-95"
          style={{
            borderColor: "var(--border)",
            width: compact ? 64 : 104,
            height: compact ? 64 : 78,
          }}
          title={attachment.name}
          aria-label={`Preview ${attachment.name}`}
        >
          <img
            src={src}
            alt={attachment.name}
            loading="lazy"
            className="h-full w-full object-cover"
            onError={() => setFailed(true)}
          />
        </button>
        {onRemove ? (
          <button
            type="button"
            onClick={() => onRemove(attachment.id)}
            className="focus-ring absolute -top-1.5 -right-1.5 flex h-5 w-5 items-center justify-center rounded-full border shadow-sm"
            style={{
              background: "var(--panel)",
              borderColor: "var(--border)",
              color: "var(--muted)",
            }}
            title={`Remove ${attachment.name}`}
            aria-label={`Remove ${attachment.name}`}
          >
            <X size={10} />
          </button>
        ) : null}
      </div>
    );
  }

  return (
    <div
      className={cn(
        "relative flex min-w-0 items-center gap-2 rounded-lg border px-2.5 py-1.5",
        compact ? "max-w-[220px]" : "max-w-[260px]",
      )}
      style={{ borderColor: "var(--border)", background: "var(--panel-2)" }}
      title={attachment.name}
    >
      <span
        className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md"
        style={{ background: "var(--panel)", color: "var(--muted)" }}
      >
        <AttachmentIcon attachment={attachment} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[11.5px] font-medium">
          {attachment.name}
        </span>
        <span className="block truncate text-[9.5px]" style={{ color: "var(--faint)" }}>
          {formatBytes(attachment.size)}
          {attachment.kind === "text" && attachment.truncated ? " · truncated" : ""}
          {attachment.kind === "file" ? " · binary" : ""}
        </span>
      </span>
      {onRemove ? (
        <button
          type="button"
          onClick={() => onRemove(attachment.id)}
          className="focus-ring -mr-1 rounded p-1 transition hover:bg-[var(--panel)]"
          style={{ color: "var(--muted)" }}
          title={`Remove ${attachment.name}`}
          aria-label={`Remove ${attachment.name}`}
        >
          <X size={11} />
        </button>
      ) : (
        <button
          type="button"
          onClick={onPreview}
          className="focus-ring shrink-0 rounded p-1 text-[10px] transition hover:bg-[var(--panel)]"
          style={{ color: "var(--accent)" }}
          aria-label={`View ${attachment.name}`}
        >
          View
        </button>
      )}
    </div>
  );
}
