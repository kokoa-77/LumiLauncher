import { Component, type ErrorInfo, type ReactNode, useEffect } from "react";
import { useRouteError } from "react-router";
import i18n from "@/lib/i18n";
import { reportError } from "@/lib/report";
import { SpotArt } from "./art/SpotArt";
import { Button } from "./ui/Button";

interface State {
  failed: boolean;
}

/** 渲染出错时显示兜底页面，不白屏。 */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    reportError(error, `界面渲染错误${info.componentStack ?? ""}`);
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return <ErrorFallback />;
  }
}

/** 路由里的页面出错时（react-router 自己接住的错误）也用同一个兜底页面，并写进日志。 */
export function RouteErrorFallback() {
  const error = useRouteError();
  useEffect(() => {
    reportError(error, "页面渲染错误");
  }, [error]);
  return <ErrorFallback />;
}

function ErrorFallback() {
  return (
    <div className="grid h-full place-items-center p-8">
      <div className="flex max-w-sm flex-col items-center gap-3 text-center">
        <SpotArt kind="error" />
        <p className="font-semibold text-lg">{i18n.t("error.boundaryTitle")}</p>
        <p className="text-fg-muted text-sm">{i18n.t("error.boundaryHint")}</p>
        <Button
          variant="primary"
          onClick={() => {
            window.location.hash = "#/";
            window.location.reload();
          }}
        >
          {i18n.t("error.reload")}
        </Button>
      </div>
    </div>
  );
}
