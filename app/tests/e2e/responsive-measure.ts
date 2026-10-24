/**
 * The layout checks behind responsive.spec.ts: run in the page, they list what
 * overflows the viewport, what a clipping ancestor cuts off, undersized tap
 * targets and stretched pictures.
 */
import type { Page } from '@playwright/test';

/** Problems found in the page, as readable strings. Runs in the browser. */
export async function measure(page: Page, tapMin: number): Promise<string[]> {
  return page.evaluate(
    ({ tapMin }) => {
      const vw = window.innerWidth;
      const problems: string[] = [];
      const describe = (el: Element) => {
        const id = el.id ? `#${el.id}` : '';
        const cls = typeof el.className === 'string' && el.className ? `.${el.className.trim().split(/\s+/).slice(0, 2).join('.')}` : '';
        const label = (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
        const attrs = ['type', 'name', 'aria-label'].map((a) => (el.getAttribute(a) ? `[${a}=${el.getAttribute(a)}]` : '')).join('');
        return `${el.tagName.toLowerCase()}${id}${cls}${attrs} "${label}"`;
      };
      const docOverflow = document.documentElement.scrollWidth - vw;
      if (docOverflow > 0) problems.push(`page scrolls sideways by ${docOverflow}px`);

      const selector =
        'button, input:not([type="checkbox"]):not([type="radio"]):not([type="hidden"]), select, textarea, ' +
        '[role="button"], [role="tab"], [role="combobox"], [role="switch"], a.btn, a.nav-link, .bottom-nav a, .nav a';
      for (const el of Array.from(document.querySelectorAll<HTMLElement>(selector))) {
        const rect = el.getBoundingClientRect();
        const style = getComputedStyle(el);
        if (rect.width <= 2 || rect.height <= 2) continue;
        if (style.visibility === 'hidden' || style.display === 'none' || Number(style.opacity) === 0) continue;
        if (el.closest('[hidden], [aria-hidden="true"]')) continue;
        const name = describe(el);
        if (rect.left < -0.5 || rect.right > vw + 0.5) {
          problems.push(`outside viewport (${Math.round(rect.left)}..${Math.round(rect.right)} of ${vw}): ${name}`);
        }
        // An overflow-hidden ancestor that cuts the control off.
        let anc = el.parentElement;
        while (anc && anc !== document.body) {
          const s = getComputedStyle(anc);
          if (['hidden', 'clip'].includes(s.overflowX) || ['hidden', 'clip'].includes(s.overflowY)) {
            const a = anc.getBoundingClientRect();
            if (rect.left < a.left - 0.5 || rect.right > a.right + 0.5 || rect.top < a.top - 0.5 || rect.bottom > a.bottom + 0.5) {
              problems.push(`clipped by overflow-hidden ${describe(anc)}: ${name}`);
              break;
            }
          }
          anc = anc.parentElement;
        }
        if (tapMin > 0 && Math.min(rect.width, rect.height) < tapMin) {
          problems.push(`tap target ${Math.round(rect.width)}x${Math.round(rect.height)} < ${tapMin}: ${name}`);
        }
      }
      // Anything laid out past the viewport edge, unless a scrollable box of its own holds it.
      const scrollable = (el: Element) => {
        for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
          const s = getComputedStyle(p);
          if (['auto', 'scroll'].includes(s.overflowX)) return true;
        }
        return false;
      };
      for (const el of Array.from(document.querySelectorAll<HTMLElement>('main *, header *, .bottom-nav *, .bottom-more *'))) {
        const rect = el.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) continue;
        if (getComputedStyle(el).display === 'none' || getComputedStyle(el).visibility === 'hidden') continue;
        if (rect.right <= vw + 0.5 && rect.left >= -0.5) continue;
        if (scrollable(el)) continue;
        problems.push(`overflows viewport (${Math.round(rect.left)}..${Math.round(rect.right)} of ${vw}): ${describe(el)}`);
      }

      // Pictures keep their aspect ratio (a logo or an avatar is never stretched).
      for (const img of Array.from(document.querySelectorAll<HTMLImageElement>('main img, header img'))) {
        const rect = img.getBoundingClientRect();
        if (!img.complete || img.naturalWidth === 0 || rect.width <= 0 || rect.height <= 0) continue;
        if (getComputedStyle(img).objectFit !== 'fill' || rect.width < 16 || rect.height < 16) continue;
        const natural = img.naturalWidth / img.naturalHeight;
        const shown = rect.width / rect.height;
        if (Math.abs(shown - natural) / natural > 0.05) {
          problems.push(`image stretched (${Math.round(rect.width)}x${Math.round(rect.height)} vs ${img.naturalWidth}x${img.naturalHeight}): ${describe(img)}`);
        }
      }
      return problems;
    },
    { tapMin },
  );
}
