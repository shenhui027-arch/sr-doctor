import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

function manualChunks(id: string): string | undefined {
  const normalized = id.replaceAll("\\", "/");
  if (/\/node_modules\/(?:react|react-dom|scheduler)\//.test(normalized)) return "react-vendor";
  if (/\/node_modules\/@xyflow\//.test(normalized)) return "graph-vendor";
  return undefined;
}

export default defineConfig({
  plugins: [react()],
  // The layout worker (created with type: "module") loads ELK as its own chunk.
  worker: { format: "es" },
  build: {
    rollupOptions: {
      output: { manualChunks },
    },
  },
  server: {
    host: "127.0.0.1",
  },
});
