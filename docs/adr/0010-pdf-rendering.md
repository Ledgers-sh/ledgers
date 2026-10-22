# 0010: PDF rendering and document anchors

Date: 2026-10-22. Status: accepted.

## Context

Ledgers stores source documents (invoices, receipts, bank statements) as blobs
and reads their content for policy and extraction. The app and agents need to
view the originals and highlight semantic fields (invoice number, total, line
items) for verification. Rendering PDFs on the server (to PNG/JPEG) is common,
but Workers have no PDF rasterizer library available and storage would double
(original + image). PDF.js (Mozilla's client-side PDF renderer) is the
production standard; the browser and CLI have JavaScript runtimes.

## Decision

Render PDFs on the client with **pdf.js**, pinned and self-hosted per ADR 0007
conventions, from the signed short-lived download link. Anchors (field
locations) are normalized to 1/10000 units of page dimensions (renderer-
independent); the client scales them to the rendered page's pixel size (e.g.,
A4 at 96 DPI = 1240×1754 px) and draws overlays or highlights. Images (PNG,
JPEG) are shown natively with the same anchor format. No server-side
rasterization.

## Consequences

- **No PDF library on Workers.** The hosting platform (Cloudflare Workers) has
  no native PDF parser or rasterizer; a Java/Node.js worker process would add
  cost and latency.
- **Anchors are compact and format-agnostic.** A bounding box in 1/10000 units
  works for both PDF.js (vector) and image (raster), and for any client DPI or
  zoom level; XML paths target structured documents (CII/ZUGFeRD e-invoices).
- **Link expiry protects the original.** Documents are signed and expire; they
  cannot be replayed or downloaded without a fresh signature.
- **Client rendering is standard practice.** PDF.js is used by Firefox, Google
  Docs, GitHub and many others; the library is audited and performant.
- **BillReview preview changes.** The existing `<iframe>` that shows a page
  image is replaced when anchoring lands; pdf.js renders the PDF in a canvas
  or iframe, and overlays are drawn with browser canvas APIs.
- **Structured documents render as fields.** E-invoice XML (UBL, CII/ZUGFeRD)
  is rendered by the extraction step (mapping fields to named anchors); the
  viewer shows the rendered text with xml-path anchors highlighting the source
  nodes.

## Why not page images

Pre-rendering PDFs to images trades:

| Aspect | Page images | PDF.js |
|--------|-------------|--------|
| **Storage** | Original + raster (2×) | Original only |
| **Rendering time** | Server (ms to sec per page) | Client (instant, cached) |
| **Interactivity** | Static; copy/select text? No | Full PDF interactivity |
| **Platforms** | Needs worker process | Browser/CLI JavaScript |
| **Zoom/rotation** | Re-render (slow) | Client canvas (instant) |

For Ledgers, PDF.js wins: storage is constrained, client interactivity aids
verification, and the hosting platform forbids background workers.
