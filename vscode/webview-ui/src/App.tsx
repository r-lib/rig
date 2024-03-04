import { vscode } from "./utilities/vscode";
import { VSCodeButton } from "@vscode/webview-ui-toolkit/react";
import { useState } from 'react';
import "./App.css";

interface IRVersion {
  name: string;
  default: boolean;
  version: string;
  aliases: Array<string>;
  path: string;
};

function RVersion() {
  return <div></div>;
}

function RVersionList() {
  const [versions, setVersions] = useState<Array<IRVersion>>();
  return <div></div>;
}

window.addEventListener("load", main);

function main() {
  // Handle the message inside the webview
  window.addEventListener('message', event => {
    const message = event.data; // The JSON data our extension sent
    switch (message.command) {
    case 'versions':
      const v = document.getElementById('versions');
      if (!!v) {
        v.textContent = JSON.stringify(message.data);
      }
      break;
    }
  });
}

function App() {
  function refreshClick() {
    vscode.postMessage({
      command: "refresh",
      text: "Reloading installed R versions",
    });
  }

  return (
    <main>
      <h1>Installalled R versions</h1>
      <RVersionList />
      <VSCodeButton onClick={refreshClick}>Refresh</VSCodeButton>
    </main>
  );
}

export default App;
