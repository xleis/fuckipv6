import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 5_000,
    },
  },
});

// Expose the commit the bundle was built from: the release gate requires the
// artifact to be traceable, and the embedded single binary ships this bundle.
document.documentElement.dataset.buildCommit = __FUCKIPV6_BUILD_COMMIT__;

const root = document.getElementById("root");
if (!root) throw new Error("WebUI root element is missing");

createRoot(root).render(
  <QueryClientProvider client={queryClient}>
    <App />
  </QueryClientProvider>,
);
