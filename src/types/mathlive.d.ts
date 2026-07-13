import type { DetailedHTMLProps, HTMLAttributes } from "react";

declare module "react" {
  namespace JSX {
    interface IntrinsicElements {
      "math-field": DetailedHTMLProps<HTMLAttributes<HTMLElement>, HTMLElement> & {
        value?: string;
        "virtual-keyboard-mode"?: "manual" | "onfocus" | "off";
        "smart-mode"?: boolean;
      };
    }
  }
}
