import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { installScrollbarAutoHide } from "./lib/scrollbars";
import { initTheme } from "./lib/theme";
import { QueryProvider } from "./queries";
import "./index.css";

installScrollbarAutoHide();
initTheme();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryProvider>
      <App />
    </QueryProvider>
  </StrictMode>,
);
