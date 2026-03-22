import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  define: {
    "process.env": {},
  },
  server: {
    port: 3011,
    proxy: {
      "/api": "http://localhost:3010",
    },
  },
});
