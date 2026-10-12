import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { AppearanceProvider } from "./AppearanceProvider";
import "./style.css";
import "./appearance.css";
import "./settings.css";
import "./browser.css";

// Suppress WebView menus (reload, lookup, translation, etc.) across the app,
// including portal content and editable fields. Custom menu handlers still run.
document.addEventListener("contextmenu", (event) => event.preventDefault(), {
  capture: true,
});

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AppearanceProvider>
      <App />
    </AppearanceProvider>
  </React.StrictMode>,
);
