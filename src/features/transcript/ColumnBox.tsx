/**
 * Where the conversation column lies, centered, as wide as 「设置 › 通用 › 对话宽度」 (narrower when the pane is):
 * marks it for what is placed beside it (the outline sits in the margin to its left).
 */
export function ColumnBox() {
  return (
    <div
      data-testid="conversation-column"
      className="pointer-events-none absolute inset-y-0 left-1/2 -translate-x-1/2"
      style={{ width: "min(var(--conv-w), calc(100% - 48px))" }}
    />
  );
}
