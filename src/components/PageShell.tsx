import type { ReactNode } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

interface PageShellProps {
  header: ReactNode;
  children: ReactNode;
  bodyClassName?: string;
  headerClassName?: string;
}

export default function PageShell({
  header,
  children,
  bodyClassName,
  headerClassName,
}: PageShellProps) {
  return (
    <div className="flex flex-1 flex-col overflow-hidden">
      <div className={cn(
        "shrink-0 border-b border-white/[0.06] bg-background/95 px-5 py-5 backdrop-blur-sm",
        headerClassName,
      )}
      >
        {header}
      </div>

      <ScrollArea className="flex-1">
        <div className={cn("space-y-4 p-5 pb-5", bodyClassName)}>
          {children}
        </div>
      </ScrollArea>
    </div>
  );
}
