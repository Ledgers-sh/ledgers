# 0009: Overview charts: hand-drawn SVG, no chart library

Date: 2026-10-23. Status: accepted.

## Context

The Overview needs a KPI strip with sparklines, a 12-month revenue and expense
chart with a net line, a cash chart, a ranked expense list, a dimension split
and an agent-versus-person activity chart. Every chart must be keyboard and
screen-reader accessible, follow the app's dark and paper themes, mirror in
Arabic (the time axis stays left to right) and respect reduced motion. The app
is a PWA that must load fast on a poor connection, and the bundle budget for
this change is +40 kB gzip.

## Decision

Draw the charts as SVG components in `app/src/ui/charts.tsx`, with the scale
maths in `app/src/lib/scale.ts` and the trend maths in `app/src/lib/trends.ts`.
No chart library is added.

Measured on this change (gzip, production build): JavaScript 83.8 kB before,
97.6 kB after (+13.9 kB); CSS 19.6 kB before, 22.0 kB after (+2.4 kB). Total
+16.3 kB, inside the 40 kB budget.

## Why not a library

uPlot (about 50 kB minified, about 15 kB gzip, pinned) would cover the line and
bar cases, but the Overview also needs bars with a line, stacked bars,
sparklines and a ranked list, and every one of them has to carry the same
focus model and hidden data table. Wrapping a canvas library in those would
cost about as much code as drawing the few shapes directly, and a canvas cannot
expose its numbers to assistive technology without a parallel table anyway.
SVG keeps each mark a DOM node that the tests and the screen reader can see.

## Consequences

- Every chart is a `figure` with a text summary and a visually hidden data
  table. The keyboard target is a focusable overlay: arrows move a crosshair
  across the months, and the tooltip repeats the exact amounts.
- Geometry is plain numbers. Money stays in BigInt minor units until the moment
  it is drawn; percent change and runway are display ratios rounded to one
  decimal and unit tested.
- Animation is CSS only and is switched off under `prefers-reduced-motion`.
- The charts are not canvas-fast. At 12 to 36 months they draw in a few
  milliseconds; revisit if a range of hundreds of months is ever shown.
