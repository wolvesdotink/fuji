import { fileUrl } from "@/lib/commands";

// Module-level cache — persists across component instances for the session
const preloadedPaths = new Set<string>();

export function useHoverPreload() {
  let timer: number | null = null;

  function startPreload(filePath: string) {
    if (preloadedPaths.has(filePath)) return;
    cancelPreload();
    timer = window.setTimeout(() => {
      const img = new Image();
      img.src = fileUrl(filePath);
      img.onload = () => preloadedPaths.add(filePath);
    }, 300);
  }

  function cancelPreload() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  return { startPreload, cancelPreload };
}
