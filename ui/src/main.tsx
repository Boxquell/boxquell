import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { api } from "./api";
import "./styles.css";

// The desktop app's window has no tabs and cannot open links itself:
// hand links that leave the interface to the user's browser.
if (navigator.userAgent.includes("Boxquell-Desktop")) {
  document.addEventListener("click", (event) => {
    const link = (event.target as Element | null)?.closest?.("a[target=_blank]") as HTMLAnchorElement | null;
    if (link && /^https?:/.test(link.href)) {
      event.preventDefault();
      api.openLink(link.href).catch(() => {});
    }
  });
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
