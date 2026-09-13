import { useState } from "react";
import { createRoot } from "react-dom/client";
import { PreviewPane } from "../../src/components/PreviewPane";
import "../../src/styles/global.css";

function samplePdf() {
  const streams = ["1 0 0 rg 80 50 200 100 re f BT /F1 20 Tf 50 230 Td (Setwright PDF verification) Tj ET", "0 0 1 rg 80 50 200 100 re f BT /F1 20 Tf 50 230 Td (Second page) Tj ET"];
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 420 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 420 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ...streams.map(s=>`<< /Length ${s.length} >>\nstream\n${s}\nendstream`),
  ];
  let content = "%PDF-1.4\n";
  const offsets = [0];
  objects.forEach((obj,i)=>{offsets.push(content.length);content+=`${i+1} 0 obj\n${obj}\nendobj\n`;});
  const start = content.length;
  content += `xref\n0 ${objects.length+1}\n0000000000 65535 f \n${offsets.slice(1).map(o=>`${String(o).padStart(10,'0')} 00000 n \n`).join('')}trailer\n<< /Size ${objects.length+1} /Root 1 0 R >>\nstartxref\n${start}\n%%EOF`;
  return new TextEncoder().encode(content);
}
const pdf = samplePdf();
const invalid = new TextEncoder().encode("Invalid PDF fixture");
function Fixture() {
  const [state,setState]=useState("ready");
  return <div data-theme="light" style={{height:"100vh",display:"flex",flexDirection:"column"}}>
    <nav aria-label="Fixture states">{["ready","stale","failed","invalid"].map(value=><button key={value} onClick={()=>setState(value)}>{value}</button>)}</nav>
    <PreviewPane pdfBytes={state==="invalid"?invalid:pdf} stale={state==="stale"} compileStatus={state==="failed"?"failed":"success"} runtimeReason="Verification fixture: compilation is unavailable." projectTitle="Verification fixture" />
  </div>;
}
createRoot(document.getElementById("root")).render(<Fixture/>);
