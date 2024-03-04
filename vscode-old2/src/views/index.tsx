import { createRoot } from "react-dom/client";
import { WebviewApi, WithWebviewContext } from "./WebviewContext";
import { RigList } from "./RigList";

export const Views = {
  rigList: RigList,
} as const;

export type ViewKey = keyof typeof Views;

export function render<V extends ViewKey>(
  key: V,
  vscodeApi: WebviewApi,
  publicPath: string,
  rootId = "root"
) {
  const container = document.getElementById(rootId);
  if (!container) {
    throw new Error(`Element with id of ${rootId} not found.`);
  }

  const Component: React.ComponentType = Views[key];

  const root = createRoot(container);

  root.render(
    <WithWebviewContext vscodeApi={vscodeApi}>
      <Component />
    </WithWebviewContext>
  );
}
