// The terminal window (ADR-0015): `terminal.html?holder=<Holder ID>`, opened from the card of a screen ccrpc
// cannot read. Its own entry, sharing nothing with the main window but the stylesheet.

import { createRoot } from "react-dom/client";
import { TerminalPage } from "./features/terminal/TerminalPage";
import "./index.css";

const params = new URLSearchParams(window.location.search);
const holderId = params.get("holder") ?? "";
// The tauri window has its title already; the mock's popup takes it from the URL.
const title = params.get("title");
if (title) document.title = `终端 · ${title}`;

// No StrictMode: its second mount would attach again while the first attach is still under way, and the
// connection that ends up kept could be the one feeding the terminal already disposed.
createRoot(document.getElementById("root")!).render(<TerminalPage holderId={holderId} />);
