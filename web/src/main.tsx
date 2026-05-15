import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter/index.css";
import "./index.css";
import App from "./App.tsx";

// Super is dark-native (SUPER.md §1). Keep `.dark` applied so any
// next-themes / Tailwind variants that target it continue to resolve.
if (typeof window !== "undefined") {
  document.documentElement.classList.add("dark");
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
