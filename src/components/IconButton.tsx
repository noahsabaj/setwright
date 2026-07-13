import type { ReactNode } from "react";
import { Button } from "react-aria-components";

interface IconButtonProps {
  label: string;
  children: ReactNode;
  active?: boolean;
  disabled?: boolean;
  badge?: string;
  className?: string;
  onPress: () => void;
}

export function IconButton({
  label,
  children,
  active = false,
  disabled = false,
  badge,
  className = "",
  onPress,
}: IconButtonProps) {
  return (
    <Button
      className={`icon-button ${className}`}
      aria-label={label}
      aria-pressed={active}
      isDisabled={disabled}
      onPress={onPress}
    >
      {children}
      {badge === undefined ? null : <span className="icon-button__badge">{badge}</span>}
    </Button>
  );
}
