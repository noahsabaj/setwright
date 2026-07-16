const encoder = new TextEncoder();

export const PDF_PROBE_FIXTURE_SHA256 = "137a07612d530b29102095fdc6685439f430a91cc4df26ff19c416297e858c54";

function byteLength(value: string): number {
  return encoder.encode(value).byteLength;
}

/**
 * Builds the small, deterministic PDF used to prove that the installed webview
 * can start the real PDF.js worker and paint a canvas. It intentionally uses
 * only vector operators, so the result has no font, image-decoder, timestamp,
 * locale, or host-runtime dependency.
 */
export function createPdfProbeFixture(): Uint8Array {
  const redPage = "1 1 1 rg 0 0 200 200 re f\n1 0 0 rg 20 20 160 160 re f\n";
  const bluePage = "1 1 1 rg 0 0 200 200 re f\n0 0 1 rg 20 20 160 160 re f\n";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R >>",
    `<< /Length ${String(byteLength(redPage))} >>\nstream\n${redPage}endstream`,
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 6 0 R >>",
    `<< /Length ${String(byteLength(bluePage))} >>\nstream\n${bluePage}endstream`,
  ];

  let pdf = "%PDF-1.4\n%\u00e2\u00e3\u00cf\u00d3\n";
  const offsets = [0];
  for (const [index, object] of objects.entries()) {
    offsets.push(byteLength(pdf));
    pdf += `${String(index + 1)} 0 obj\n${object}\nendobj\n`;
  }

  const xrefOffset = byteLength(pdf);
  pdf += `xref\n0 ${String(objects.length + 1)}\n`;
  pdf += "0000000000 65535 f \n";
  for (const offset of offsets.slice(1)) {
    pdf += `${String(offset).padStart(10, "0")} 00000 n \n`;
  }
  pdf += `trailer\n<< /Size ${String(objects.length + 1)} /Root 1 0 R >>\n`;
  pdf += `startxref\n${String(xrefOffset)}\n%%EOF\n`;
  return encoder.encode(pdf);
}
