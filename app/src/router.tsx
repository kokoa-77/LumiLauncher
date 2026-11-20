import { createHashRouter } from "react-router";
import { RouteErrorFallback } from "./components/ErrorBoundary";

import { AppShell } from "./layouts/AppShell";
import { HomePage } from "./pages/HomePage";

export const router = createHashRouter([
  {
    path: "/",
    element: <AppShell />,
    errorElement: <RouteErrorFallback />,
    children: [
      { index: true, element: <HomePage /> },
      {
        path: "instances",
        lazy: async () => ({ Component: (await import("./pages/InstancesPage")).InstancesPage }),
      },
      {
        path: "instances/:id",
        lazy: async () => ({
          Component: (await import("./pages/InstanceDetailPage")).InstanceDetailPage,
        }),
      },
      {
        path: "discover",
        lazy: async () => ({ Component: (await import("./pages/DiscoverPage")).DiscoverPage }),
      },
      {
        path: "discover/:source/:projectId",
        lazy: async () => ({ Component: (await import("./pages/ProjectPage")).ProjectPage }),
      },
      // 组件展示页：只在开发版里有，正式包里这一项和整个页面都会被删掉
      ...(import.meta.env.DEV
        ? [
            {
              path: "dev/components",
              lazy: async () => ({
                Component: (await import("./dev/ComponentsPage")).ComponentsPage,
              }),
            },
          ]
        : []),
    ],
  },
]);
