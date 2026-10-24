import type { ComponentChildren, Ref } from 'preact';
import { useEffect, useRef } from 'preact/hooks';

/**
 * A row of tabs that scrolls sideways on a phone. When it overflows, the
 * active tab is brought into view (centred), and a fade on the cut edge shows
 * that the row goes on. The fade follows the reading direction (RTL flips it).
 */
export function TabRow({
  active,
  label,
  tablist = false,
  class: className,
  children,
}: {
  /** The selected tab's value: a change re-centres the row on the new tab. */
  active: string;
  label: string;
  /** A row of buttons with role tablist; otherwise a nav of links. */
  tablist?: boolean;
  class?: string;
  children: ComponentChildren;
}) {
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    const row = ref.current;
    if (!row) return;
    const update = () => {
      const max = row.scrollWidth - row.clientWidth;
      const moved = Math.abs(row.scrollLeft);
      const before = moved > 1;
      const after = max - moved > 1;
      row.dataset.fade = before && after ? 'both' : before ? 'start' : after ? 'end' : '';
    };
    row.addEventListener('scroll', update, { passive: true });
    window.addEventListener('resize', update);
    row.querySelector<HTMLElement>('[aria-selected="true"], [aria-current="page"]')?.scrollIntoView({ block: 'nearest', inline: 'center' });
    update();
    return () => {
      row.removeEventListener('scroll', update);
      window.removeEventListener('resize', update);
    };
  }, [active]);

  const classes = ['tabs', className].filter(Boolean).join(' ');
  if (tablist) {
    return (
      <div ref={ref as Ref<HTMLDivElement>} class={classes} role="tablist" aria-label={label}>
        {children}
      </div>
    );
  }
  return (
    <nav ref={ref as Ref<HTMLElement>} class={classes} aria-label={label}>
      {children}
    </nav>
  );
}
