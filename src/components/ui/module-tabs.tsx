import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export interface ModuleTabItem<T extends string = string> {
  id: T;
  label: string;
  icon?: LucideIcon;
  badge?: string | number;
}

interface ModuleTabsProps<T extends string> {
  items: ModuleTabItem<T>[];
  value: T;
  onValueChange: (value: T) => void;
  className?: string;
}

export default function ModuleTabs<T extends string>({
  items,
  value,
  onValueChange,
  className,
}: ModuleTabsProps<T>) {
  return (
    <div className={cn("overflow-x-auto pb-1", className)}>
      <div className="flex w-max min-w-full gap-1 rounded-2xl border border-white/[0.08] bg-white/[0.03] p-1">
        {items.map((item) => {
          const active = item.id === value;
          const Icon = item.icon;

          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => onValueChange(item.id)}
              className={cn(
                "flex min-w-[136px] items-center justify-center gap-2 whitespace-nowrap rounded-xl px-3.5 py-2.5 text-[13px] font-medium transition-all",
                active
                  ? "bg-background text-foreground shadow-[0_8px_24px_rgba(0,0,0,0.24)]"
                  : "text-muted-foreground hover:bg-white/[0.04] hover:text-foreground",
              )}
            >
              {Icon ? <Icon size={15} className={active ? "text-foreground" : "text-muted-foreground"} /> : null}
              <span>{item.label}</span>
              {item.badge !== undefined ? (
                <span
                  className={cn(
                    "rounded-full px-1.5 py-0.5 text-[10px] leading-none",
                    active ? "bg-white/[0.08] text-foreground/80" : "bg-white/[0.06] text-muted-foreground",
                  )}
                >
                  {item.badge}
                </span>
              ) : null}
            </button>
          );
        })}
      </div>
    </div>
  );
}
