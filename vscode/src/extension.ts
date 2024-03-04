import * as vscode from 'vscode';
import { rigPanel } from "./panels/rigPanel";

export function activate(context: vscode.ExtensionContext) {
  let currentPanel: vscode.WebviewPanel | undefined = undefined;

  const showRigCommand = vscode.commands.registerCommand("rig.showRig", () => {
    rigPanel.render(context.extensionUri);
  });

  context.subscriptions.push(showRigCommand);
}
