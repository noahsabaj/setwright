# Accessibility acceptance

Setwright targets WCAG 2.2 AA for the desktop workflow. Automated rules are a
floor; platform assistive-technology testing is a release gate.

## Interaction requirements

- Every command is reachable by keyboard without timing-dependent gestures.
- Pane splitters expose value, orientation, and keyboard increments.
- Scientific nodes have useful accessible names and source-mode escape hatches.
- Focus remains visible and is restored after dialogs, menus, and mode changes.
- Status updates use non-stealing live regions; compile logs do not repeatedly
  interrupt typing.
- Write, Source, Preview, and Split layouts reflow at 200% zoom without hiding
  essential controls or requiring two-dimensional scrolling for text.
- Light, dark, and high-contrast themes preserve meaning without color alone.
- Reduced-motion preferences disable nonessential motion.

## Release matrix

Manual end-to-end checks cover NVDA with WebView2 on Windows, VoiceOver with
WKWebView on macOS, and Orca with WebKitGTK on Linux. Test paper creation,
visual/source navigation, math and citation insertion, diagnostics, compile,
PDF navigation, comments/suggestions, conflict resolution, and export.

Record OS, webview, screen-reader versions, failures, and retest evidence with
the release candidate. A passing automated audit does not substitute for this
matrix.
