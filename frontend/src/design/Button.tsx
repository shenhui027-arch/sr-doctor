import type { ButtonHTMLAttributes, JSX } from "react";
import { Icon, type IconName } from "./ui";

export function Button(
  props: ButtonHTMLAttributes<HTMLButtonElement> & {
    variant?: "primary" | "secondary" | "warning" | "danger-outline" | "danger" | "ghost";
    size?: "md" | "sm";
    icon?: IconName;
  },
): JSX.Element {
  const {
    variant = "secondary",
    size = "md",
    icon,
    className,
    children,
    type = "button",
    ...rest
  } = props;
  const classes = ["btn", `btn-${variant}`, size === "sm" ? "btn--sm" : "", className ?? ""].filter(Boolean).join(" ");
  return (
    <button className={classes} type={type} {...rest}>
      {icon ? <Icon name={icon} size={size === "sm" ? 14 : 15} /> : null}
      {children}
    </button>
  );
}
