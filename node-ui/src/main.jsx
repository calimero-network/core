import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App.jsx";
import "./styles/index.css";
import "react-tooltip/dist/react-tooltip.css";
import { HeliaProvider } from "./provider/HeliaProvider";

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <HeliaProvider>
      <App />
    </HeliaProvider>
  </React.StrictMode>
);
