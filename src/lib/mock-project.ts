import type { ProjectSnapshot } from "./contracts";

export const demoLatexSource = String.raw`\documentclass[11pt]{article}
\usepackage{amsmath,amssymb,booktabs,graphicx}
\usepackage[margin=1in]{geometry}

\title{Retrieval-Augmented Models Under Distribution Shift}
\author{Maya Chen \and Noah Williams}
\date{}

\begin{document}
\maketitle

\begin{abstract}
Retrieval augmentation improves factuality, but its behavior under distribution shift remains poorly understood. We introduce a controlled evaluation spanning temporal, geographic, and lexical shifts.
\end{abstract}

\section{Introduction}
Large language models can rely on external evidence at inference time \cite{lewis2020rag}. We ask when retrieval remains useful as the query distribution moves away from the training corpus.

\begin{equation}
  p(y \mid x) = \sum_{z \in \mathcal{Z}_k(x)} p_\eta(z \mid x) p_\theta(y \mid x,z)
  \label{eq:rag-objective}
\end{equation}

\input{sections/method}

\section{Results}
Across three shifts, evidence-conditioned models retain 87\% of their in-domain performance.

\begin{table}[t]
  \centering
  \caption{Accuracy under distribution shift.}
  \label{tab:results}
  \begin{tabular}{lcc}
    \toprule
    Model & In-domain & Shifted \\
    \midrule
    Parametric & 76.4 & 58.9 \\
    Ours & \textbf{82.1} & \textbf{71.4} \\
    \bottomrule
  \end{tabular}
\end{table}

\bibliographystyle{plain}
\bibliography{references}
\end{document}
`;

export const demoProject: ProjectSnapshot = {
  sessionId: "demo-session",
  revision: 14,
  rootPath: "~/Papers/distribution-shift",
  title: "Retrieval-Augmented Models Under Distribution Shift",
  mainFile: "file-main",
  files: [
    {
      id: "file-main",
      relativePath: "main.tex",
      kind: "tex",
      encoding: "utf8",
      content: demoLatexSource,
      dirty: false,
      byteLength: new TextEncoder().encode(demoLatexSource).length,
      sha256: "demo-main-sha256",
    },
    {
      id: "file-method",
      relativePath: "sections/method.tex",
      kind: "tex",
      encoding: "utf8",
      content: "\\section{Method}\nWe evaluate three controlled shifts across five retrieval corpora.\n",
      dirty: false,
      byteLength: 84,
      sha256: "demo-method-sha256",
    },
    {
      id: "file-bib",
      relativePath: "references.bib",
      kind: "bib",
      encoding: "utf8",
      content: "@article{lewis2020rag, title={Retrieval-Augmented Generation}, year={2020}}\n",
      dirty: false,
      byteLength: 81,
      sha256: "demo-bib-sha256",
    },
    {
      id: "file-figure",
      relativePath: "figures/shift-overview.pdf",
      kind: "asset",
      encoding: "utf8",
      content: null,
      dirty: false,
      byteLength: 0,
      sha256: "demo-asset-sha256",
    },
  ],
  settings: {
    schemaVersion: 1,
    projectId: "019f5cca-demo",
    mainFile: "main.tex",
    templateId: "generic-article",
    runtimeId: "texlive-2025-2025-08-03",
    engine: "pdflatex",
  },
  dirty: false,
  compatibility: [],
};

export function cloneDemoProject(): ProjectSnapshot {
  return structuredClone(demoProject);
}
