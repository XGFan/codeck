import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { installScrollbarAutoHide } from "./lib/scrollbars";
import "./index.css";

installScrollbarAutoHide();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
