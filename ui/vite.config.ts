import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `npm run dev` needs a running daemon: BOXQUELL_API=http://127.0.0.1:<port> npm run dev
export default defineConfig({
  plugins: [react()],
  server: { proxy: { "/api": process.env.BOXQUELL_API ?? "http://127.0.0.1:1" } },
});
