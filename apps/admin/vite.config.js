import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  // The proxy preserves the server's same-origin checks. Never expose this dev
  // server publicly: use an explicit loopback management listener for development.
  const target = env.FELUCCA_ADMIN_TARGET || "http://127.0.0.1:18080";
  const proxy = {
    target,
    changeOrigin: true,
    configure(server) {
      server.on("proxyReq", (request) => {
        if (request.getHeader("origin")) request.setHeader("origin", new URL(target).origin);
      });
    },
  };
  return {
    plugins: [react()],
    server: {
      port: 5173,
      strictPort: true,
      proxy: { "/api": proxy, "/metrics": proxy },
    },
    build: { outDir: "dist", target: "es2022", sourcemap: false },
  };
});
