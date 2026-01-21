"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = void 0;
const vscode = require("vscode");
const rigPanel_1 = require("./panels/rigPanel");
function activate(context) {
    let currentPanel = undefined;
    const showRigCommand = vscode.commands.registerCommand("rig.showRig", () => {
        rigPanel_1.rigPanel.render(context.extensionUri);
    });
    context.subscriptions.push(showRigCommand);
}
exports.activate = activate;
//# sourceMappingURL=extension.js.map