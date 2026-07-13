# First-party paper templates

Each directory is a complete ordinary LaTeX project with Setwright metadata:

- [`generic/`](generic/) uses the standard `article` class.
- [`acm/`](acm/) uses ACM's `acmart` class in anonymous review mode.
- [`ieee/`](ieee/) uses IEEE's `IEEEtran` conference class.

The projects default to the managed TeX Live 2025 profile and pdfLaTeX. Change
the engine through Setwright rather than hand-editing metadata while a project
is open. Imported projects do not receive metadata unless explicitly adopted.

Template structure is Apache-2.0 licensed as part of Setwright. Authors retain
rights to the paper content they create from it. Venue requirements change;
always compare the selected class/options and submission metadata with the
current official author instructions before submission.
