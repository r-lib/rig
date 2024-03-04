import { vscode } from "./utilities/vscode";
import { VSCodeButton } from "@vscode/webview-ui-toolkit/react";
import { useState, useCallback, useEffect } from 'react';
import "./App.css";

interface IRVersion {
  name: string;
  default: boolean;
  version: string;
  aliases: Array<string>;
  path: string;
};

function RVersionList() {
  const [versions, setVersions] = useState<Array<IRVersion>>();

  const listener = useCallback(event => {
    const message = event.data;
    switch (message.command) {
    case 'versions':
      setVersions(message.data);
      break;
    }
  }, []);

  useEffect(() => {
    window.addEventListener("message", listener);
    return () => {
        window.removeEventListener("message", listener);
    };
  }, [listener]);

  const listItems = versions?.map(v =>
    <li key={v.name}>
      <RVersion {...v} />
    </li>
  );

  return (
    <ul>{listItems}</ul>
  );
}

 function RVersion(version: IRVersion) {
   return <>
     {version.default ? "✅" : ""} {version.name} (R {version.version})
   </>
 }

function App() {
  function refreshClick() {
    vscode.postMessage({
      command: "refresh"
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
