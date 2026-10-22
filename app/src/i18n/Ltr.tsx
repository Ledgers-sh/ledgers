import type { ComponentChildren } from 'preact';

/**
 * Left-to-right run inside a right-to-left line. Amounts, codes, dates and
 * IDs keep their own order in Arabic; the isolate stops bidi reordering.
 */
export function Ltr({ children, class: className }: { children: ComponentChildren; class?: string }) {
  return (
    <bdi dir="ltr" class={className}>
      {children}
    </bdi>
  );
}
