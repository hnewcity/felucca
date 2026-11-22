import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The window only talks to the Rust side over IPC; there is no proxy.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5174, strictPort: true },
  build: { outDir: "dist", target: "es2022", sourcemap: false },
});
