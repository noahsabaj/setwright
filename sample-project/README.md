# Setwright sample paper

This synthetic, multi-file paper exercises the MVP's conservative visual
surface: metadata, abstract, static unique `\input` files, sections, citations,
cross-references, inline and display math, theorem/proof structures, a figure,
a `booktabs` table, a footnote, and a `listings` code block.

The prose and measurements are fictional and must not be presented as research
results. The sample is Apache-2.0 licensed with the rest of the repository.

Compile in a trusted TeX environment with:

```sh
latexmk -pdf -norc main.tex
```

Setwright compilation should be used only after its managed runtime and native
sandbox gates are complete.
