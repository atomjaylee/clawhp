import { useEffect } from "react";
import { CircleAlert, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";

interface ConfirmActionDialogProps {
  open: boolean;
  title: string;
  description: string;
  confirmLabel?: string;
  cancelLabel?: string;
  busy?: boolean;
  destructive?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmActionDialog({
  open,
  title,
  description,
  confirmLabel = "确认",
  cancelLabel = "取消",
  busy = false,
  destructive = false,
  onConfirm,
  onCancel,
}: ConfirmActionDialogProps) {
  useEffect(() => {
    if (!open) {
      return undefined;
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        onCancel();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [busy, onCancel, open]);

  if (!open) {
    return null;
  }

  return (
    <div
      className="fixed inset-0 z-[120] flex items-center justify-center bg-black/70 px-4 backdrop-blur-sm"
      onClick={() => {
        if (!busy) {
          onCancel();
        }
      }}
    >
      <Card
        className="w-full max-w-md border-white/[0.08] bg-[#081017] shadow-2xl shadow-black/40"
        onClick={(event) => event.stopPropagation()}
      >
        <CardContent className="p-5">
          <div className="flex items-start gap-3">
            <div className="mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-destructive/10 text-destructive">
              <CircleAlert size={18} />
            </div>
            <div className="min-w-0 flex-1">
              <h3 className="text-sm font-semibold text-foreground">{title}</h3>
              <p className="mt-2 text-[12px] leading-6 text-muted-foreground">{description}</p>
            </div>
          </div>

          <div className="mt-5 flex items-center justify-end gap-2">
            <Button type="button" variant="outline" onClick={onCancel} disabled={busy}>
              {cancelLabel}
            </Button>
            <Button
              type="button"
              variant={destructive ? "destructive" : "default"}
              onClick={onConfirm}
              disabled={busy}
            >
              {busy ? <Loader2 className="animate-spin" /> : null}
              {busy ? "处理中..." : confirmLabel}
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
