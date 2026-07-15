import React, { useRef, useState } from "react";
import { X } from "lucide-react";
import { Tooltip } from "../ui/Tooltip";

interface ShortcutClearButtonProps {
  label: string;
  disabled: boolean;
  onClick: () => void;
}

export const ShortcutClearButton: React.FC<ShortcutClearButtonProps> = ({
  label,
  disabled,
  onClick,
}) => {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const [showTooltip, setShowTooltip] = useState(false);

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        aria-label={label}
        disabled={disabled}
        onClick={onClick}
        onMouseEnter={() => setShowTooltip(true)}
        onMouseLeave={() => setShowTooltip(false)}
        onFocus={() => setShowTooltip(true)}
        onBlur={() => setShowTooltip(false)}
        className="p-1 rounded-md border border-transparent transition-all duration-150 hover:bg-logo-primary/30 hover:border-logo-primary disabled:opacity-50 disabled:cursor-not-allowed"
      >
        <X className="h-4 w-4" aria-hidden="true" />
      </button>
      {showTooltip && (
        <Tooltip targetRef={buttonRef} position="top">
          {label}
        </Tooltip>
      )}
    </>
  );
};
