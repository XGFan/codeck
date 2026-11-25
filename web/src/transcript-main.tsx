import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import TranscriptPage from "./TranscriptPage";
import { installScrollbarAutoHide } from "./lib/scrollbars";
import { initTheme } from "./lib/theme";
import { QueryProvider } from "./queries";
import "./index.css";

installScrollbarAutoHide();
initTheme();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryProvider>
      <TranscriptPage />
    </QueryProvider>
  </StrictMode>,
);
