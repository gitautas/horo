import React from "react";
import ReactDOM from "react-dom/client";

// Self-hosted fonts and icons, bundled so the app works offline and satisfies
// the CSP, which forbids remote stylesheets.
// Baloo 2 — display · Unbounded — rave · Nunito — body · JetBrains Mono — code
import "@fontsource/baloo-2/500.css";
import "@fontsource/baloo-2/600.css";
import "@fontsource/baloo-2/700.css";
import "@fontsource/baloo-2/800.css";
import "@fontsource/unbounded/500.css";
import "@fontsource/unbounded/700.css";
import "@fontsource/unbounded/800.css";
import "@fontsource/unbounded/900.css";
import "@fontsource-variable/nunito";
import "@fontsource-variable/jetbrains-mono";
import "@phosphor-icons/web/fill";

import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { installSelectionClamp } from "./lib/selection";
import "./styles/index.css";

installSelectionClamp();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
