import { useState } from "react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { connFor, isLive, useConnect } from "@/features/connect";
import { api } from "@/ipc";
import { cn } from "@/lib/cn";
import { isComposingKey, keepOpenWhileComposing } from "@/lib/hotkeys";
import { useUi, type RenameTarget } from "@/state/ui";
import { BUTTON } from "./AddProjectDialog";

/** 「重命名」 of a Connected Session: `/rename <title>`, so the name is Claude Code's own. */
export function RenameDialog() {
  const target = useUi((s) => s.renaming);
  const setRenaming = useUi((s) => s.setRenaming);
  const close = () => setRenaming(null);
  return (
    <Dialog open={target != null} onOpenChange={(o) => !o && close()}>
      <DialogContent
        data-testid="rename-dialog"
        aria-describedby={undefined}
        onEscapeKeyDown={keepOpenWhileComposing}
        className="w-[420px]"
      >
        <DialogTitle>重命名 Session</DialogTitle>
        {target && <RenameForm key={target.id} target={target} onClose={close} />}
      </DialogContent>
    </Dialog>
  );
}

function RenameForm({ target, onClose }: { target: RenameTarget; onClose: () => void }) {
  const command = useConnect((s) => s.command);
  const [text, setText] = useState(target.title);
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  // ccrpc takes a slash command on one line only.
  const title = text.replace(/\s+/g, " ").trim();

  const submit = async () => {
    if (!title || sending) return;
    if (title === target.title) return onClose();
    const c = connFor(useConnect.getState(), target.id);
    if (!c || !isLive(c)) return setError("重命名失败：Session 已断开");
    setSending(true);
    setError(null);
    const r = await command(c.conn, { type: "slash", command: `rename ${title}` });
    if (r.success && c.sessionId) void api.syncSession(c.sessionId).catch(() => {});
    // Cancelled while it was sent (up to 3 s): the dialog may be open for another Session by now.
    if (useUi.getState().renaming !== target) return;
    setSending(false);
    if (!r.success) return setError(`重命名失败：${r.error?.message ?? r.error?.code ?? ""}`);
    onClose();
  };

  return (
    <>
      <Input
        data-testid="rename-input"
        aria-label="Session 标题"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onFocus={(e) => e.currentTarget.select()}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !isComposingKey(e.nativeEvent)) void submit();
        }}
      />
      {error && (
        <div role="alert" data-testid="rename-error" className="mt-2 text-[12px] text-error">
          {error}
        </div>
      )}
      <div className="mt-3 flex justify-end gap-2">
        <button type="button" onClick={onClose} className={BUTTON}>
          取消
        </button>
        <button
          type="button"
          disabled={!title || sending}
          onClick={() => void submit()}
          className={cn(BUTTON, "border-accent bg-accent font-semibold text-white disabled:opacity-40")}
        >
          重命名
        </button>
      </div>
    </>
  );
}
