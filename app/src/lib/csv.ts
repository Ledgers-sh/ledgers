/** Builds RFC 4180 CSV text. Cells that start with a formula character are prefixed to stay inert in spreadsheets. */
export function toCsv(rows: (string | number)[][]): string {
  return rows
    .map((row) =>
      row
        .map((cell) => {
          let text = String(cell);
          if (/^[=+\-@\t\r]/.test(text)) text = `'${text}`;
          return /[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
        })
        .join(','),
    )
    .join('\r\n');
}
