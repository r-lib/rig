import * as vscode from 'vscode';
import { ViewKey } from "./views";
import { registerView } from "./registerView";
import {
  ViewApi,
  ViewApiError,
  ViewApiRequest,
  ViewApiResponse,
} from "./viewApi";
import fs from "node:fs/promises";

// This method is called when your extension is activated
// Your extension is activated the very first time the command is executed
export function activate(context: vscode.ExtensionContext) {
	const connectedViews: Partial<Record<ViewKey, vscode.WebviewView>> = {};

	// Use the console to output diagnostic information (console.log) and errors (console.error)
	// This line of code will only be executed once when your extension is activated
	console.log('Congratulations, your extension "rig" is now active!');

	const api: ViewApi = {
		getFileContents: async () => {
		  const uris = await vscode.window.showOpenDialog({
			canSelectFiles: true,
			canSelectFolders: false,
			canSelectMany: false,
			openLabel: "Select file",
			title: "Select file to read",
		  });

		  if (!uris?.length) {
			return "";
		  }

		  const contents = await fs.readFile(uris[0].fsPath, "utf-8");
		  return contents;
		},
		showRigList: () => {
		  connectedViews?.rigList?.show?.(true);
		  vscode.commands.executeCommand(`rigList.focus`);
		},
	  };

	  const isViewApiRequest = <K extends keyof ViewApi>(
		msg: unknown
	  ): msg is ViewApiRequest<K> =>
		msg != null &&
		typeof msg === "object" &&
		"type" in msg &&
		msg.type === "request";

	  const registerAndConnectView = async <V extends ViewKey>(key: V) => {
		const view = await registerView(context, key);
		connectedViews[key] = view;
		const onMessage = async (msg: Record<string, unknown>) => {
		  if (!isViewApiRequest(msg)) {
			return;
		  }
		  try {
			const val = await Promise.resolve(api[msg.key](...msg.params));
			const res: ViewApiResponse = {
			  type: "response",
			  id: msg.id,
			  value: val,
			};
			view.webview.postMessage(res);
		  } catch (e: unknown) {
			const err: ViewApiError = {
			  type: "error",
			  id: msg.id,
			  value:
				e instanceof Error ? e.message : "An unexpected error occurred",
			};
			view.webview.postMessage(err);
		  }
		};

		view.webview.onDidReceiveMessage(onMessage);
	  };

	// The command has been defined in the package.json file
	// Now provide the implementation of the command with registerCommand
	// The commandId parameter must match the command field in package.json
	let rigManageCmd = vscode.commands.registerCommand('rig.manage', () => {
		// The code you place here will be executed every time your command is executed
		// Display a message box to the user
		vscode.window.showInformationMessage('HelloWorld from rig!');
		console.log(connectedViews.rigList);
		connectedViews?.rigList?.show?.(true);
		vscode.commands.executeCommand(`rigList.focus`);
	});

	context.subscriptions.push(rigManageCmd);
	registerAndConnectView("rigList");
}

// This method is called when your extension is deactivated
export function deactivate() {}
