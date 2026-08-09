import { useEffect, useRef } from "react";

export function Markdown({
  html,
  className,
  onOpenLink,
}: {
  html: string;
  className?: string;
  onOpenLink: (url: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onClick = (e: MouseEvent) => {
      const target = (e.target as HTMLElement).closest?.("a");
      const href = target?.getAttribute("href");
      if (target && href && /^https?:\/\//i.test(href)) {
        e.preventDefault();
        onOpenLink(href);
      }
    };
    el.addEventListener("click", onClick);
    return () => el.removeEventListener("click", onClick);
  }, [onOpenLink]);

  return (
    <div
      ref={ref}
      className={className}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
