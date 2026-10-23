import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const css = readFileSync(fileURLToPath(new URL('./styles.css', import.meta.url)), 'utf8');

// Layout classes every screen leans on. A second top-level rule for one of them (a chart once
// reused `.stack` as a 14px bar) silently clips forms across the app, so each is defined once.
const LAYOUT = ['stack', 'panel', 'panel-body', 'field', 'row-actions'];

describe('styles.css', () => {
  it.each(LAYOUT)('defines the layout class .%s exactly once', (name) => {
    const rules = css.match(new RegExp(`^\\.${name} \\{`, 'gm')) ?? [];
    expect(rules).toHaveLength(1);
  });
});
