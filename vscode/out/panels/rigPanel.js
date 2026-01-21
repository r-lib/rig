"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.rigPanel = void 0;
const vscode_1 = require("vscode");
const getUri_1 = require("../utilities/getUri");
const getNonce_1 = require("../utilities/getNonce");
const child_process_1 = require("child_process");
const util_1 = require("util");
const asyncExecFile = (0, util_1.promisify)(child_process_1.execFile);
const sudo_prompt_1 = require("@vscode/sudo-prompt");
function listRVersions() {
    return __awaiter(this, void 0, void 0, function* () {
        const out = yield asyncExecFile("rig", ["ls", "--json"]);
        const versions = JSON.parse(out.stdout);
        return versions;
    });
}
function installRVersion(version) {
    return __awaiter(this, void 0, void 0, function* () {
        const options = { name: 'rig' };
        (0, sudo_prompt_1.exec)("rig add " + version, options, function (error, stdout, stderr) {
            if (error) {
                throw error;
            }
            console.log(stdout);
        });
    });
}
class rigPanel {
    constructor(panel, extensionUri) {
        this._disposables = [];
        this._panel = panel;
        // Set an event listener to listen for when the panel is disposed (i.e. when the user closes
        // the panel or when the panel is closed programmatically)
        this._panel.onDidDispose(() => this.dispose(), null, this._disposables);
        // Set the HTML content for the webview panel
        this._panel.webview.html = this._getWebviewContent(this._panel.webview, extensionUri);
        // Set an event listener to listen for messages passed from the webview context
        this._setWebviewMessageListener(this._panel.webview);
    }
    /**
     * Renders the current webview panel if it exists otherwise a new webview panel
     * will be created and displayed.
     *
     * @param extensionUri The URI of the directory containing the extension.
     */
    static render(extensionUri) {
        return __awaiter(this, void 0, void 0, function* () {
            if (rigPanel.currentPanel) {
                // If the webview panel already exists reveal it
                rigPanel.currentPanel._panel.reveal(vscode_1.ViewColumn.One);
            }
            else {
                // If a webview panel does not already exist create and show a new one
                const panel = vscode_1.window.createWebviewPanel(
                // Panel view type
                "showRig", 
                // Panel title
                "Manage R installations with rig", 
                // The editor column the panel should be displayed in
                vscode_1.ViewColumn.One, 
                // Extra panel configurations
                {
                    // Enable JavaScript in the webview
                    enableScripts: true,
                    // Restrict the webview to only load resources from the `out` and `webview-ui/build` directories
                    localResourceRoots: [vscode_1.Uri.joinPath(extensionUri, "out"), vscode_1.Uri.joinPath(extensionUri, "webview-ui/build")],
                });
                rigPanel.currentPanel = new rigPanel(panel, extensionUri);
            }
            const rvers = yield listRVersions();
            rigPanel.currentPanel._panel.webview.postMessage({ command: "versions", data: rvers });
        });
    }
    /**
     * Cleans up and disposes of webview resources when the webview panel is closed.
     */
    dispose() {
        rigPanel.currentPanel = undefined;
        // Dispose of the current webview panel
        this._panel.dispose();
        // Dispose of all disposables (i.e. commands) for the current webview panel
        while (this._disposables.length) {
            const disposable = this._disposables.pop();
            if (disposable) {
                disposable.dispose();
            }
        }
    }
    /**
     * Defines and returns the HTML that should be rendered within the webview panel.
     *
     * @remarks This is also the place where references to the React webview build files
     * are created and inserted into the webview HTML.
     *
     * @param webview A reference to the extension webview
     * @param extensionUri The URI of the directory containing the extension
     * @returns A template string literal containing the HTML that should be
     * rendered within the webview panel
     */
    _getWebviewContent(webview, extensionUri) {
        // The CSS file from the React build output
        const stylesUri = (0, getUri_1.getUri)(webview, extensionUri, [
            "webview-ui",
            "build",
            "static",
            "css",
            "main.css",
        ]);
        // The JS file from the React build output
        const scriptUri = (0, getUri_1.getUri)(webview, extensionUri, [
            "webview-ui",
            "build",
            "static",
            "js",
            "main.js",
        ]);
        const nonce = (0, getNonce_1.getNonce)();
        // Tip: Install the es6-string-html VS Code extension to enable code highlighting below
        return /*html*/ `
      <!DOCTYPE html>
      <html lang="en">
        <head>
          <meta charset="utf-8">
          <meta name="viewport" content="width=device-width,initial-scale=1,shrink-to-fit=no">
          <meta name="theme-color" content="#000000">
          <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';">
          <link rel="stylesheet" type="text/css" href="${stylesUri}">
          <title>Manage R installations with rig</title>
        </head>
        <body>
          <noscript>You need to enable JavaScript to run this app.</noscript>
          <div id="root"></div>
          <div id="versions"></div>
          <script nonce="${nonce}" src="${scriptUri}"></script>
          <script nonce="${nonce}">
          </script>
        </body>
      </html>
    `;
    }
    /**
     * Sets up an event listener to listen for messages passed from the webview context and
     * executes code based on the message that is recieved.
     *
     * @param webview A reference to the extension webview
     * @param context A reference to the extension context
     */
    _setWebviewMessageListener(webview) {
        webview.onDidReceiveMessage((message) => __awaiter(this, void 0, void 0, function* () {
            var _a, _b, _c, _d, _e, _f;
            const command = message.command;
            const text = message.text;
            if (text) {
                vscode_1.window.showInformationMessage(text);
            }
            switch (command) {
                case "refresh":
                    {
                        const rvers = yield listRVersions();
                        (_c = (_b = (_a = rigPanel === null || rigPanel === void 0 ? void 0 : rigPanel.currentPanel) === null || _a === void 0 ? void 0 : _a._panel) === null || _b === void 0 ? void 0 : _b.webview) === null || _c === void 0 ? void 0 : _c.postMessage({ command: "versions", data: rvers });
                        return;
                    }
                case "install":
                    {
                        console.log(message);
                        const out = yield installRVersion(message.version);
                        const rvers = yield listRVersions();
                        (_f = (_e = (_d = rigPanel === null || rigPanel === void 0 ? void 0 : rigPanel.currentPanel) === null || _d === void 0 ? void 0 : _d._panel) === null || _e === void 0 ? void 0 : _e.webview) === null || _f === void 0 ? void 0 : _f.postMessage({ command: "versions", data: rvers });
                        return;
                    }
            }
        }), undefined, this._disposables);
    }
}
exports.rigPanel = rigPanel;
//# sourceMappingURL=rigPanel.js.map