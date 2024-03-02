import * as vscode from "vscode";
import { randomBytes } from "crypto";
import path from "node:path";
import { ViewKey } from "./views";

const DEV_SERVER_HOST = "http://localhost:18080";

const template = (params: {
  csp: string;
  view: ViewKey;
  srcUri: string;
  publicPath: string;
  title: string;
  nonce: string;
}) => `
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>${params.title}</title>
    <meta http-equiv="Content-Security-Policy" content="${params.csp}" />
  </head>

  <body>
    <div id="root"></div>
    <script type="module" nonce="${params.nonce}">
      import { render } from "${params.srcUri}";
      render("${params.view}", acquireVsCodeApi(), "${params.publicPath}");
    </script>
  </body>
</html>
`;

const createView = async <V extends ViewKey>(
  ctx: vscode.ExtensionContext,
  viewId: V,
  options?: vscode.WebviewOptions
): Promise<vscode.WebviewView> => {
  return await new Promise((resolve, reject) => {
    let dispose: vscode.Disposable;
    try {
      const provider: vscode.WebviewViewProvider = {
        resolveWebviewView: (view, _viewCtx, _token) => {
          try {
            view.onDidDispose(() => {
              dispose.dispose();
            });
            view.webview.options = { ...options };
            resolve(view);
          } catch (err: unknown) {
            reject(err);
          }
        },
      };
      dispose = vscode.window.registerWebviewViewProvider(viewId, provider);
      ctx.subscriptions.push(dispose);
    } catch (err: unknown) {
      reject(err);
    }
  });
};

const setViewHtml = <V extends ViewKey>(
  ctx: vscode.ExtensionContext,
  viewId: V,
  webview: vscode.Webview
) => {
  const isProduction = ctx.extensionMode === vscode.ExtensionMode.Production;
  const nonce = randomBytes(16).toString("base64");

  const uri = (...parts: string[]) =>
    webview
      .asWebviewUri(vscode.Uri.file(path.join(ctx.extensionPath, ...parts)))
      .toString(true);

  const publicPath = isProduction ? uri() : `${DEV_SERVER_HOST}/`;
  const srcUri = isProduction ? uri("views.js") : `${DEV_SERVER_HOST}/views.js`;

  const csp = (
    isProduction
      ? [
          `form-action 'none';`,
          `default-src ${webview.cspSource};`,
          `script-src ${webview.cspSource} 'nonce-${nonce}';`,
          `style-src ${webview.cspSource} ${DEV_SERVER_HOST} 'unsafe-inline';`,
        ]
      : [
          `form-action 'none';`,
          `default-src ${webview.cspSource} ${DEV_SERVER_HOST};`,
          `style-src ${webview.cspSource} ${DEV_SERVER_HOST} 'unsafe-inline';`,
          `script-src ${webview.cspSource} ${DEV_SERVER_HOST} 'nonce-${nonce}';`,
          `connect-src 'self' ${webview.cspSource} ${DEV_SERVER_HOST} ws:;`,
        ]
  ).join(" ");

  webview.html = template({
    title: "Example",
    csp,
    srcUri,
    publicPath,
    view: viewId,
    nonce,
  });
  return webview;
};

export const registerView = async <V extends ViewKey>(
  ctx: vscode.ExtensionContext,
  viewId: V
) => {
  const view = await createView(ctx, viewId, { enableScripts: true });
  setViewHtml(ctx, viewId, view.webview);
  return view;
};