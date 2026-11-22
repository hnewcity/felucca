import React from "react";
import { createRoot } from "react-dom/client";
import "./styles/tokens.css";
import "./styles/app.css";
import "./styles/monitor.css";
import App from "./app/App.jsx";
import { LocaleProvider } from "./app/locale.jsx";
import { FatalError } from "./components/FatalError.jsx";

createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <LocaleProvider>
      <FatalError>
        <App />
      </FatalError>
    </LocaleProvider>
  </React.StrictMode>,
);
