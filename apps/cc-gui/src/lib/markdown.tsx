import type { ComponentProps } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { CopyButton } from "@/components/ui/copy-button";
import { cn } from "@/lib/cn";
import { HighlightedCode } from "@/lib/shiki";

const components: Components = {
  p: (p) => <p className="my-0 [&:not(:last-child)]:mb-2" {...strip(p)} />,
  h1: (p) => <h3 className="mt-3 mb-1 text-[15px] font-semibold" {...strip(p)} />,
  h2: (p) => <h3 className="mt-3 mb-1 text-[14px] font-semibold" {...strip(p)} />,
  h3: (p) => <h4 className="mt-2 mb-1 text-[13px] font-semibold" {...strip(p)} />,
  h4: (p) => <h4 className="mt-2 mb-1 text-[13px] font-semibold" {...strip(p)} />,
  ul: (p) => <ul className="my-1 list-disc space-y-0.5 pl-5" {...strip(p)} />,
  ol: (p) => <ol className="my-1 list-decimal space-y-0.5 pl-5" {...strip(p)} />,
  blockquote: (p) => <blockquote className="my-2 border-l-2 border-border pl-3 text-secondary" {...strip(p)} />,
  a: (p) => <a className="text-accent underline-offset-2 hover:underline" target="_blank" rel="noreferrer" {...strip(p)} />,
  hr: () => <hr className="my-3 border-border" />,
  table: (p) => (
    <div className="my-2 overflow-x-auto">
      <table className="border-collapse text-[12px]" {...strip(p)} />
    </div>
  ),
  th: (p) => <th className="border border-border bg-code px-2 py-1 text-left font-semibold" {...strip(p)} />,
  td: (p) => <td className="border border-border px-2 py-1" {...strip(p)} />,
  pre: ({ node, ...rest }) => {
    const first = node?.children[0];
    const raw = first && "children" in first ? first.children.map((c) => ("value" in c ? c.value : "")).join("") : "";
    return (
      <div className="group/copy relative my-2">
        <pre
          className="overflow-x-auto rounded-md bg-code px-3 py-2 font-mono text-[12px] leading-[1.55] whitespace-pre"
          {...rest}
        />
        <CopyButton text={raw.replace(/\n$/, "")} className="absolute top-1.5 right-1.5 bg-code/90" />
      </div>
    );
  },
  code: ({ className, children, node: _node, ...rest }) => {
    const lang = /language-([\w+#-]+)/.exec(className ?? "")?.[1] ?? null;
    const block = lang != null || String(children).includes("\n");
    return block ? (
      <code className={className} {...rest}>
        <HighlightedCode code={String(children).replace(/\n$/, "")} lang={lang} />
      </code>
    ) : (
      <code className="rounded bg-code px-[5px] py-px font-mono text-[12px]" {...rest}>
        {children}
      </code>
    );
  },
};

function strip<T extends { node?: unknown }>({ node: _node, ...rest }: T) {
  return rest;
}

export function Markdown({ text, className }: { text: string; className?: string }) {
  return (
    <div className={cn("min-w-0 break-words", className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}

export type MarkdownProps = ComponentProps<typeof Markdown>;
